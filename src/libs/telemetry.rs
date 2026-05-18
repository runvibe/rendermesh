use anyhow::Result;
use opentelemetry::{global, trace::TracerProvider as _};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator, resource::Resource, trace::SdkTracerProvider,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(tracer_provider) = self.tracer_provider.as_ref() {
            let _ = tracer_provider.shutdown();
        }
    }
}

impl TelemetryGuard {
    pub fn force_flush(&self) -> Result<()> {
        if let Some(tracer_provider) = self.tracer_provider.as_ref() {
            tracer_provider.force_flush().map_err(Into::into)
        } else {
            Ok(())
        }
    }
}

pub fn init_tracing(enabled: bool) -> Result<TelemetryGuard> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive("otel::tracing=info".parse().expect("valid directive"));

    if !enabled {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .compact(),
            )
            .init();

        return Ok(TelemetryGuard {
            tracer_provider: None,
        });
    }

    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource();
    let exporter = build_exporter()?;
    let use_simple = std::env::var("OTEL_USE_SIMPLE_EXPORTER")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let builder = SdkTracerProvider::builder().with_resource(resource);
    let tracer_provider = if use_simple {
        builder.with_simple_exporter(exporter).build()
    } else {
        builder.with_batch_exporter(exporter).build()
    };
    global::set_tracer_provider(tracer_provider.clone());

    let tracer = tracer_provider.tracer(env!("CARGO_PKG_NAME"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .compact(),
        )
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();

    Ok(TelemetryGuard {
        tracer_provider: Some(tracer_provider),
    })
}

fn build_exporter() -> Result<SpanExporter> {
    let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
        .unwrap_or_else(|_| "grpc".to_string())
        .to_ascii_lowercase();

    match protocol.as_str() {
        "http/protobuf" => build_http_exporter(Protocol::HttpBinary),
        "http/json" => build_http_exporter(Protocol::HttpJson),
        "grpc" => SpanExporter::builder()
            .with_tonic()
            .build()
            .map_err(Into::into),
        _ => SpanExporter::builder()
            .with_tonic()
            .build()
            .map_err(Into::into),
    }
}

fn build_http_exporter(protocol: Protocol) -> Result<SpanExporter> {
    SpanExporter::builder()
        .with_http()
        .with_protocol(protocol)
        .build()
        .map_err(Into::into)
}

fn build_resource() -> Resource {
    let mut builder = Resource::builder();
    if std::env::var("OTEL_SERVICE_NAME").is_err() {
        builder = builder.with_service_name(env!("CARGO_PKG_NAME"));
    }
    builder.build()
}
