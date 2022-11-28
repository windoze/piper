use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures::{pin_mut, Future};
use pyo3::exceptions::PyException;
use pyo3::types::PyTuple;
use pyo3::{create_exception, prelude::*};
use tokio::runtime::Handle;

create_exception!(piper, PiperError, PyException);

/**
 * Check CTRL-C every 100ms, cancel the future if pressed and return Interrupted error
 */
async fn cancelable_wait<F, T: Send>(f: F) -> PyResult<T>
where
    F: Future<Output = PyResult<T>>,
{
    // Future needs to be pinned then its mutable ref can be awaited multiple times.
    pin_mut!(f);
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(100), &mut f).await {
            Ok(v) => {
                return v;
            }
            Err(_) => {
                // Timeout, check if CTRL-C is pressed
                Python::with_gil(|py| py.check_signals())?
            }
        }
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future),
    }
}
struct Value(piper::Value);

impl<'source> FromPyObject<'source> for Value {
    fn extract(ob: &PyAny) -> PyResult<Self> {
        if ob.is_none() {
            return Ok(Value(piper::Value::Null));
        }
        if let Ok(v) = ob.extract::<bool>() {
            Ok(Value(piper::Value::Bool(v)))
        } else if let Ok(v) = ob.extract::<i64>() {
            Ok(Value(piper::Value::Long(v)))
        } else if let Ok(v) = ob.extract::<f64>() {
            Ok(Value(piper::Value::Double(v)))
        } else if let Ok(v) = ob.extract::<String>() {
            Ok(Value(piper::Value::from(v)))
        } else if let Ok(v) = ob.extract::<DateTime<Utc>>() {
            Ok(Value(piper::Value::DateTime(v)))
        } else if let Ok(v) = ob.extract::<Vec<Value>>() {
            Ok(Value(v.into_iter().map(|v| v.0).collect()))
        } else if let Ok(v) = ob.extract::<HashMap<String, Value>>() {
            let m = piper::Value::Object(v.into_iter().map(|(k, v)| (k, v.0)).collect());
            Ok(Value(m))
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "Unsupported type",
            ))
        }
    }
}

impl ToPyObject for Value {
    fn to_object(&self, py: Python<'_>) -> PyObject {
        match &self.0 {
            piper::Value::Null => py.None(),
            piper::Value::Bool(v) => v.to_object(py),
            piper::Value::Int(v) => v.to_object(py),
            piper::Value::Long(v) => v.to_object(py),
            piper::Value::Float(v) => v.to_object(py),
            piper::Value::Double(v) => v.to_object(py),
            piper::Value::String(v) => v.as_ref().to_object(py),
            piper::Value::Array(v) => v
                .iter()
                .map(|v| Value(v.clone()))
                .collect::<Vec<_>>()
                .to_object(py),
            piper::Value::Object(v) => v
                .iter()
                .map(|(k, v)| (k.clone(), Value(v.clone())))
                .collect::<HashMap<_, _>>()
                .to_object(py),
            piper::Value::DateTime(v) => v.to_object(py),
            piper::Value::Error(v) => PyErr::new::<PiperError, _>(v.to_string()).to_object(py),
        }
    }
}

#[derive(Clone)]
struct PyPiperFunction {
    function: PyObject,
}

impl piper::Function for PyPiperFunction {
    fn get_output_type(
        &self,
        _argument_types: &[piper::ValueType],
    ) -> Result<piper::ValueType, piper::PiperError> {
        Ok(piper::ValueType::Dynamic)
    }

    fn eval(&self, arguments: Vec<piper::Value>) -> piper::Value {
        Python::with_gil(|py| {
            let args = PyTuple::new(py, arguments.into_iter().map(|v| Value(v).to_object(py)));
            self.function
                .call1(py, args)
                .map(|v| {
                    v.extract::<Value>(py).map(|v| v.0).unwrap_or_else(|e| {
                        piper::Value::Error(piper::PiperError::ExternalError(e.to_string()))
                    })
                })
                .unwrap_or_else(|e| {
                    piper::Value::Error(piper::PiperError::ExternalError(e.to_string()))
                })
        })
    }
}

#[pyclass]
#[pyo3(text_signature = "(pipelines lookups functions /)")]
struct PiperService {
    service: piper::PiperService,
}

#[pymethods]
impl PiperService {
    #[new]
    #[args(lookups = "\"\"", functions = "HashMap::new()")]
    fn new(pipelines: &str, lookups: &str, functions: HashMap<String, PyObject>) -> Self {
        let functions = functions
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    Box::new(PyPiperFunction { function: v }) as Box<dyn piper::Function>,
                )
            })
            .collect();

        Self {
            service: piper::PiperService::create(pipelines, lookups, functions),
        }
    }

    #[pyo3(text_signature = "($self address port /)")]
    fn start<'p>(&self, address: &str, port: u16, py: Python<'p>) -> PyResult<()> {
        py.allow_threads(|| {
            block_on(cancelable_wait(async {
                self.service
                    .start_at(address, port)
                    .await
                    .map_err(|e| PyErr::new::<PiperError, _>(e.to_string()))
            }))
        })
    }
}

#[pymodule]
#[pyo3(name = "feathrpiper")]
fn python(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<PiperService>()?;
    Ok(())
}
