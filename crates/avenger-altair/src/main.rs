//! `avenger-altair validate spec.json` · `avenger-altair render spec.json out.svg|out.png`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let spec = std::fs::read_to_string(args.get(2).expect("a spec file")).expect("readable");
    let dir = std::path::Path::new(&args[2]).parent().and_then(|p| p.to_str()).map(String::from);
    match args.get(1).map(String::as_str) {
        Some("validate") => match avenger_altair::validate(&spec) {
            Ok(_) => println!("valid for Avenger"),
            Err(r) => {
                println!("{r}");
                std::process::exit(1);
            }
        },
        Some("render") => {
            let out = args.get(3).expect("an output path");
            let format = match out.rsplit('.').next() {
                Some("png") => avenger_altair::Format::Png,
                _ => avenger_altair::Format::Svg,
            };
            let n: usize = args.get(4).and_then(|n| n.parse().ok()).unwrap_or(1);
            let mut st = avenger_altair::Stages::default();
            let mut sum = avenger_altair::Stages::default();
            let mut result = None;
            for _ in 0..n {
                result = Some(avenger_altair::render_timed(&spec, format, 2.0, dir.as_deref(), &Default::default(), &mut st));
                (sum.validate, sum.compile, sum.render, sum.export) = (sum.validate + st.validate, sum.compile + st.compile, sum.render + st.render, sum.export + st.export);
            }
            let k = n as f64;
            match result.unwrap() {
                Ok(b) => {
                    std::fs::write(out, b).unwrap();
                    println!(
                        "{out}: validate {:.2} ms, compile {:.1} ms, render {:.1} ms, export {:.1} ms (mean of {n})",
                        sum.validate / k,
                        sum.compile / k,
                        sum.render / k,
                        sum.export / k
                    );
                }
                Err(r) => {
                    println!("{r}");
                    std::process::exit(1);
                }
            }
        }
        Some("scene") => match avenger_altair::scene_outline(&spec) {
            Ok(o) => print!("{o}"),
            Err(r) => println!("{r}"),
        },
        _ => eprintln!("usage: avenger-altair validate spec.json | render spec.json out.svg | scene spec.json"),
    }
}
