//! The browser module (wasm-bindgen): layers 0 to 3 by default; layer 4
//! needs DataFusion in wasm, which is not built here.
//!
//! ```js
//! import init, {Validator, exportCel} from './avenger_validate.js'
//! await init()
//! const v = new Validator()
//! v.check('read t.laz ! chart bar --x label:N ! color green').issues
//! ```

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Validator {
    inner: crate::Validator,
}

impl Default for Validator {
    fn default() -> Self {
        Validator::new()
    }
}

#[wasm_bindgen]
impl Validator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Validator {
        Validator { inner: crate::Validator::default() }
    }

    /// Check a pipeline given as text; returns `{valid, steps, layers, issues}`.
    pub fn check(&self, text: &str) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.check(text)).map_err(|e| e.to_string().into())
    }

    /// Check a pipeline given as data: `[{step, args, flags}]`.
    #[wasm_bindgen(js_name = checkSteps)]
    pub fn check_steps(&self, steps: JsValue) -> Result<JsValue, JsValue> {
        let v: serde_json::Value = serde_wasm_bindgen::from_value(steps).map_err(|e| JsValue::from(e.to_string()))?;
        serde_wasm_bindgen::to_value(&self.inner.check_json(&v)).map_err(|e| e.to_string().into())
    }

    /// The text as steps: `[{step, args, flags}]`.
    pub fn parse(&self, text: &str) -> Result<JsValue, JsValue> {
        let calls = crate::parse(text).map_err(|e| JsValue::from(e.message))?;
        serde_wasm_bindgen::to_value(&crate::calls_to_json(&calls)).map_err(|e| e.to_string().into())
    }
}

/// Layers 1 and 2 as a CEL bundle (JSON text).
#[wasm_bindgen(js_name = exportCel)]
pub fn export_cel() -> String {
    crate::export::cel_bundle(&crate::Spec::builtin()).to_string()
}
