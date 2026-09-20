//! Replays a LiDAR tile as a live sensor feed over Arrow Flight.
//!
//! The tile's points carry a GPS timestamp, so the flight lines can be
//! replayed in the order the scanner actually recorded them. Each `DoGet`
//! sends Arrow batches paced by that timestamp; the gaps between flight lines
//! (the plane turning around) are collapsed to one second.
//!
//! Usage: cargo run --release --bin stream_server -- <tile.copc.laz> [addr]
//!
//! The ticket is JSON: {"speed": 4.0, "from": 12.5} replays four times faster
//! than reality, starting 12.5 s into the flight. The viewer sends a new
//! ticket whenever its replay speed changes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{Array, AsArray, Float64Array, RecordBatch};
use arrow::datatypes::{DataType, Field, Float64Type, Schema, SchemaRef};
use arrow_flight::encode::FlightDataEncoderBuilder;
use arrow_flight::flight_service_server::{FlightService, FlightServiceServer};
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PollInfo, PutResult, SchemaResult, Ticket,
};
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use futures::stream::{self, BoxStream, StreamExt};
use sedona_pointcloud::las::format::{Extension, LasFormatFactory};
use sedona_pointcloud::las::options::LasOptions;
use tonic::{Request, Response, Status, Streaming};

/// Rows per Flight batch: about 0.1 s of scanning at this sensor's rate.
const BATCH_ROWS: usize = 16_384;
/// Longest gap kept between two flight lines, in seconds.
const MAX_GAP_S: f64 = 1.0;

struct Feed {
    batches: Vec<RecordBatch>,
    schema: SchemaRef,
    /// Stream time (seconds from the start) of the last row of each batch.
    ends: Vec<f64>,
    duration_s: f64,
}

/// Loads the tile, orders it by acquisition time, and builds a stream clock
/// that skips the idle time between flight lines.
async fn load(tile: &str) -> datafusion::error::Result<Feed> {
    let config = SessionConfig::new()
        .with_option_extension(LasOptions::default())
        .with_batch_size(BATCH_ROWS);
    let mut state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .build();
    state.register_file_format(Arc::new(LasFormatFactory::new(Extension::Laz)), true)?;
    let ctx = SessionContext::new_with_state(state).enable_url_table();
    ctx.sql("SET las.geometry_encoding = 'plain'").await?;

    let t = Instant::now();
    let bounds = ctx
        .sql(&format!("SELECT min(x), min(y) FROM '{tile}'"))
        .await?
        .collect()
        .await?;
    let corner = |i: usize| {
        (bounds[0].column(i).as_primitive::<Float64Type>().value(0) / 1000.0).floor() * 1000.0
    };
    let (x0, y0) = (corner(0), corner(1));

    let raw = ctx
        .sql(&format!(
            "SELECT CAST(x - {x0} AS FLOAT) AS x,
                    CAST(y - {y0} AS FLOAT) AS y,
                    CAST(z AS FLOAT) AS z,
                    CAST(classification AS INT) AS classification,
                    CAST(point_source_id AS INT) AS line,
                    gps_time
             FROM '{tile}'
             ORDER BY gps_time"
        ))
        .await?
        .collect()
        .await?;

    // Replace the absolute GPS timestamp with seconds from the start of the
    // stream, keeping real gaps inside a flight line but clamping the long
    // gaps between lines.
    let schema = Arc::new(Schema::new(vec![
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("z", DataType::Float32, false),
        Field::new("classification", DataType::Int32, false),
        Field::new("line", DataType::Int32, false),
        Field::new("t", DataType::Float64, false),
    ]));

    let (mut prev_gps, mut stream_t) = (f64::NAN, 0.0f64);
    let (mut batches, mut ends) = (Vec::new(), Vec::new());
    let mut rows = 0usize;
    for batch in &raw {
        let gps = batch.column(5).as_primitive::<Float64Type>();
        let mut ts = Vec::with_capacity(gps.len());
        for i in 0..gps.len() {
            let g = gps.value(i);
            if prev_gps.is_finite() {
                stream_t += (g - prev_gps).clamp(0.0, MAX_GAP_S);
            }
            prev_gps = g;
            ts.push(stream_t);
        }
        let cols: Vec<Arc<dyn Array>> = vec![
            Arc::new(
                batch
                    .column(0)
                    .as_primitive::<arrow::datatypes::Float32Type>()
                    .clone(),
            ) as _,
            Arc::new(
                batch
                    .column(1)
                    .as_primitive::<arrow::datatypes::Float32Type>()
                    .clone(),
            ) as _,
            Arc::new(
                batch
                    .column(2)
                    .as_primitive::<arrow::datatypes::Float32Type>()
                    .clone(),
            ) as _,
            Arc::new(
                batch
                    .column(3)
                    .as_primitive::<arrow::datatypes::Int32Type>()
                    .clone(),
            ) as _,
            Arc::new(
                batch
                    .column(4)
                    .as_primitive::<arrow::datatypes::Int32Type>()
                    .clone(),
            ) as _,
            Arc::new(Float64Array::from(ts.clone())) as _,
        ];
        rows += gps.len();
        ends.push(*ts.last().unwrap());
        batches.push(RecordBatch::try_new(schema.clone(), cols).unwrap());
    }

    println!(
        "loaded {rows} points in {} batches, {:.1} s of scanning, in {:.2?}",
        batches.len(),
        stream_t,
        t.elapsed()
    );
    Ok(Feed {
        batches,
        schema,
        ends,
        duration_s: stream_t,
    })
}

#[derive(Clone)]
struct LidarFeed {
    feed: Arc<Feed>,
}

#[tonic::async_trait]
impl FlightService for LidarFeed {
    type HandshakeStream = BoxStream<'static, Result<HandshakeResponse, Status>>;
    type ListFlightsStream = BoxStream<'static, Result<FlightInfo, Status>>;
    type DoGetStream = BoxStream<'static, Result<FlightData, Status>>;
    type DoPutStream = BoxStream<'static, Result<PutResult, Status>>;
    type DoActionStream = BoxStream<'static, Result<arrow_flight::Result, Status>>;
    type ListActionsStream = BoxStream<'static, Result<ActionType, Status>>;
    type DoExchangeStream = BoxStream<'static, Result<FlightData, Status>>;

    async fn get_flight_info(
        &self,
        _: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        let info = FlightInfo::new()
            .try_with_schema(&self.feed.schema)
            .map_err(|e| Status::internal(e.to_string()))?
            .with_endpoint(arrow_flight::FlightEndpoint::new().with_ticket(Ticket::new("{}")))
            .with_total_records(
                self.feed
                    .batches
                    .iter()
                    .map(|b| b.num_rows() as i64)
                    .sum::<i64>(),
            );
        Ok(Response::new(info))
    }

    /// Streams the tile back, paced by the sensor's own clock.
    async fn do_get(
        &self,
        request: Request<Ticket>,
    ) -> Result<Response<Self::DoGetStream>, Status> {
        let ticket = String::from_utf8_lossy(&request.into_inner().ticket).to_string();
        let params = serde_json::from_str::<serde_json::Value>(&ticket).ok();
        let field = |name: &str| {
            params
                .as_ref()
                .and_then(|v| v.get(name).and_then(|s| s.as_f64()))
        };
        let speed = field("speed").unwrap_or(1.0).max(0.01);
        let from = field("from").unwrap_or(0.0).max(0.0);
        let first = self
            .feed
            .ends
            .partition_point(|end| *end <= from)
            .min(self.feed.batches.len());
        println!(
            "client connected: {:.1} s of flight from t = {from:.1} s at {speed}x ({:.1} s of wall clock)",
            self.feed.duration_s,
            (self.feed.duration_s - from).max(0.0) / speed
        );

        let feed = self.feed.clone();
        let started = Instant::now();
        let paced = stream::iter(first..feed.batches.len()).then(move |i| {
            let feed = feed.clone();
            async move {
                // Release each batch when the scanner would have finished it,
                // counting from where this connection picks the flight up.
                let due = Duration::from_secs_f64((feed.ends[i] - from).max(0.0) / speed);
                let elapsed = started.elapsed();
                if due > elapsed {
                    tokio::time::sleep(due - elapsed).await;
                }
                Ok(feed.batches[i].clone())
            }
        });

        let flight = FlightDataEncoderBuilder::new()
            .with_schema(self.feed.schema.clone())
            .build(paced)
            .map(|r| r.map_err(|e| Status::internal(e.to_string())));
        Ok(Response::new(Box::pin(flight) as Self::DoGetStream))
    }

    async fn handshake(
        &self,
        _: Request<Streaming<HandshakeRequest>>,
    ) -> Result<Response<Self::HandshakeStream>, Status> {
        Err(Status::unimplemented("handshake"))
    }
    async fn list_flights(
        &self,
        _: Request<Criteria>,
    ) -> Result<Response<Self::ListFlightsStream>, Status> {
        Err(Status::unimplemented("list_flights"))
    }
    async fn poll_flight_info(
        &self,
        _: Request<FlightDescriptor>,
    ) -> Result<Response<PollInfo>, Status> {
        Err(Status::unimplemented("poll_flight_info"))
    }
    async fn get_schema(
        &self,
        _: Request<FlightDescriptor>,
    ) -> Result<Response<SchemaResult>, Status> {
        Err(Status::unimplemented("get_schema"))
    }
    async fn do_put(
        &self,
        _: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoPutStream>, Status> {
        Err(Status::unimplemented("do_put"))
    }
    async fn do_action(
        &self,
        _: Request<Action>,
    ) -> Result<Response<Self::DoActionStream>, Status> {
        Err(Status::unimplemented("do_action"))
    }
    async fn list_actions(
        &self,
        _: Request<Empty>,
    ) -> Result<Response<Self::ListActionsStream>, Status> {
        Err(Status::unimplemented("list_actions"))
    }
    async fn do_exchange(
        &self,
        _: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoExchangeStream>, Status> {
        Err(Status::unimplemented("do_exchange"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let tile = args
        .next()
        .expect("usage: stream_server <tile.copc.laz> [addr]");
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:50051".to_string());

    let feed = Arc::new(load(&tile).await?);
    let service = LidarFeed { feed };
    println!("Arrow Flight feed on {addr} (ticket: {{\"speed\": 4.0}})");
    tonic::transport::Server::builder()
        .add_service(FlightServiceServer::new(service))
        .serve(addr.parse()?)
        .await?;
    Ok(())
}
