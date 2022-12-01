use std::{collections::HashMap, time::Instant};

use futures::future::join_all;
use tracing::{debug, instrument};

use crate::{
    common::IgnoreDebug,
    pipeline::{BuildContext, ErrorCollector, Pipeline, PiperError, ValidationMode, Value},
    Function, Logged, Request, Response, SingleRequest, SingleResponse,
};

#[derive(Debug)]
pub struct Piper {
    pub(crate) pipelines: HashMap<String, Pipeline>,
    pub(crate) ctx: IgnoreDebug<BuildContext>,
}

impl Piper {
    pub fn new(pipeline_def: &str, lookup_def: &str) -> Result<Self, PiperError> {
        let ctx = BuildContext::from_config(lookup_def)?;

        let mut pipelines = Pipeline::load(pipeline_def, &ctx).log()?;
        // Use invalid identifier as the name, avoid clashes with user-defined pipelines
        pipelines.insert("%health".to_string(), Pipeline::get_health_checker());
        Ok(Self {
            pipelines,
            ctx: IgnoreDebug { inner: ctx },
        })
    }

    pub fn new_with_udf(
        pipeline_def: &str,
        lookup_def: &str,
        udf: HashMap<String, Box<dyn Function>>,
    ) -> Result<Self, PiperError> {
        let ctx = BuildContext::from_config_with_udf(lookup_def, udf)?;

        let mut pipelines = Pipeline::load(pipeline_def, &ctx).log()?;
        // Use invalid identifier as the name, avoid clashes with user-defined pipelines
        pipelines.insert("%health".to_string(), Pipeline::get_health_checker());
        Ok(Self {
            pipelines,
            ctx: IgnoreDebug { inner: ctx },
        })
    }

    pub async fn health_check(&self) -> bool {
        let (_, ret) = self
            .pipelines
            .get("%health")
            .unwrap()
            .process_row(vec![Value::Int(57)], ValidationMode::Strict)
            .unwrap()
            .eval()
            .await;
        if (ret.len() == 1) && (ret[0].len() == 2) {
            matches!(ret[0][1], Value::Int(99))
        } else {
            false
        }
    }

    pub fn get_pipelines(&self) -> HashMap<String, serde_json::Value> {
        self.pipelines
            .values()
            .map(|p| (p.name.clone(), p.to_json()))
            .collect()
    }

    pub fn get_lookup_sources(&self) -> serde_json::Value {
        self.ctx.inner.dump_lookup_sources()
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn process(&self, req: Request) -> Result<Response, PiperError> {
        debug!(
            "Received request, contains {} sub-requests",
            req.requests.len()
        );
        let futures: Vec<_> = req
            .requests
            .into_iter()
            .map(|r| async {
                let pipeline = r.pipeline.clone();
                let r = self.process_single_request(r).await;
                match r {
                    Ok(r) => r,
                    Err(e) => SingleResponse {
                        pipeline,
                        status: format!("ERROR: {}", e),
                        time: None,
                        count: None,
                        data: None,
                        errors: vec![],
                    },
                }
            })
            .collect();
        let results = join_all(futures).await;
        Ok(Response { results })
    }

    #[instrument(level = "trace", skip(self))]
    pub async fn process_single_request(
        &self,
        req: SingleRequest,
    ) -> Result<SingleResponse, PiperError> {
        let pipeline = self
            .pipelines
            .get(&req.pipeline)
            .ok_or_else(|| PiperError::PipelineNotFound(req.pipeline.clone()))?;
        debug!("Processing request to pipeline {}", pipeline.name);

        let schema = &pipeline.input_schema;

        let row: Vec<Value> = schema
            .columns
            .iter()
            .map(|c| {
                req.data
                    .get(c.name.as_str())
                    .map(|v| Value::from(v.clone()))
                    .unwrap_or_default()
            })
            .collect();

        let now = Instant::now();
        let (ret, errors) = pipeline
            .process_row(
                row,
                if req.validate {
                    ValidationMode::Strict
                } else {
                    ValidationMode::Lenient
                },
            )?
            .eval()
            .await
            .collect_into_json(req.errors);
        Ok(SingleResponse {
            pipeline: req.pipeline,
            status: "OK".to_owned(),
            time: Some((now.elapsed().as_micros() as f64) / 1000f64),
            count: Some(ret.len()),
            data: Some(ret),
            errors,
        })
    }
}