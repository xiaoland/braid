use std::io::Read;

use anyhow::{Context, Result, bail};
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::{KeyValue, any_value::Value as OtelValue},
};
use prost::Message;
use serde::Serialize;

use super::storage::DecodedRecord;

pub const MAX_BATCH_BYTES: usize = 16 * 1024 * 1024;

pub fn decode(signal: &str, encoding: &str, wire: &[u8]) -> Result<Vec<DecodedRecord>> {
    let payload = match encoding {
        "identity" => wire.to_vec(),
        "gzip" => {
            let mut output = Vec::new();
            flate2::read::GzDecoder::new(wire)
                .take(u64::try_from(MAX_BATCH_BYTES)? + 1)
                .read_to_end(&mut output)
                .context("decode gzip OTLP body")?;
            if output.len() > MAX_BATCH_BYTES {
                bail!("decompressed OTLP body exceeds {MAX_BATCH_BYTES} bytes");
            }
            output
        }
        _ => bail!("unsupported Content-Encoding: {encoding}"),
    };
    let mut records = Vec::new();
    match signal {
        "traces" => {
            for resource in ExportTraceServiceRequest::decode(payload.as_slice())
                .context("decode OTLP traces protobuf")?
                .resource_spans
            {
                let attrs = resource.resource.as_ref().map_or(&[][..], |r| r.attributes.as_slice());
                for scope in resource.scope_spans {
                    for span in scope.spans {
                        records.push(record(
                            attrs,
                            &resource.resource,
                            &scope.scope,
                            &span,
                            &span.attributes,
                            hex::encode(&span.trace_id),
                            hex::encode(&span.span_id),
                        )?);
                    }
                }
            }
        }
        "logs" => {
            for resource in ExportLogsServiceRequest::decode(payload.as_slice())
                .context("decode OTLP logs protobuf")?
                .resource_logs
            {
                let attrs = resource.resource.as_ref().map_or(&[][..], |r| r.attributes.as_slice());
                for scope in resource.scope_logs {
                    for log in scope.log_records {
                        records.push(record(
                            attrs,
                            &resource.resource,
                            &scope.scope,
                            &log,
                            &log.attributes,
                            hex::encode(&log.trace_id),
                            hex::encode(&log.span_id),
                        )?);
                    }
                }
            }
        }
        "metrics" => {
            for resource in ExportMetricsServiceRequest::decode(payload.as_slice())
                .context("decode OTLP metrics protobuf")?
                .resource_metrics
            {
                let attrs = resource.resource.as_ref().map_or(&[][..], |r| r.attributes.as_slice());
                for scope in resource.scope_metrics {
                    for metric in scope.metrics {
                        records.push(record(
                            attrs,
                            &resource.resource,
                            &scope.scope,
                            &metric,
                            &[],
                            String::new(),
                            String::new(),
                        )?);
                    }
                }
            }
        }
        _ => bail!("unsupported OTLP signal: {signal}"),
    }
    Ok(records)
}

fn attribute(attributes: &[KeyValue], keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            attributes.iter().find(|a| a.key == *key).and_then(|a| a.value.as_ref()).and_then(|v| {
                match &v.value {
                    Some(OtelValue::StringValue(s)) => Some(s.clone()),
                    _ => None,
                }
            })
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn record(
    resource_attributes: &[KeyValue],
    resource: &impl Serialize,
    scope: &impl Serialize,
    data: &impl Serialize,
    attributes: &[KeyValue],
    trace_id: String,
    span_id: String,
) -> Result<DecodedRecord> {
    let mut session = attribute(
        attributes,
        &[
            "braid.provider_session.id",
            "provider.thread.id",
            "braid.session.id",
            "session_id",
            "session.id",
            "provider_session_id",
        ],
    );
    if session.is_empty() {
        session = attribute(resource_attributes, &["braid.session.id", "session_id", "session.id"]);
    }
    Ok(DecodedRecord {
        instance: attribute(resource_attributes, &["service.instance.id"]),
        service: attribute(resource_attributes, &["service.name"]),
        session,
        turn_id: attribute(attributes, &["braid.turn.id", "turn_id"]),
        trace_id,
        span_id,
        resource: serde_json::to_value(resource)?,
        scope: serde_json::to_value(scope)?,
        data: serde_json::to_value(data)?,
    })
}
