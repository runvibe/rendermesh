use std::fmt::Write;

use chrono::{DateTime, SecondsFormat, Utc};
use handlebars::{
    Context, Handlebars, Helper, HelperDef, RenderContext, RenderError, RenderErrorReason,
    ScopedJson,
};
use serde_json::Value;

pub(super) fn register_date_helpers(registry: &mut Handlebars<'_>) {
    registry.register_helper("date_now", Box::new(CurrentDateHelper::Now));
    registry.register_helper("date_today", Box::new(CurrentDateHelper::Today));
    registry.register_helper("date_time", Box::new(CurrentDateHelper::Time));
    registry.register_helper("date_timestamp", Box::new(CurrentDateHelper::Timestamp));
    registry.register_helper("date_format", Box::new(DateFormatHelper));
}

enum CurrentDateHelper {
    Now,
    Today,
    Time,
    Timestamp,
}

impl HelperDef for CurrentDateHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        validate_arguments(helper, 0)?;
        let now = Utc::now();
        let value = match self {
            Self::Now => Value::String(now.to_rfc3339_opts(SecondsFormat::Secs, true)),
            Self::Today => Value::String(now.format("%Y-%m-%d").to_string()),
            Self::Time => Value::String(now.format("%H:%M:%S").to_string()),
            Self::Timestamp => Value::from(now.timestamp()),
        };
        Ok(ScopedJson::Derived(value))
    }
}

struct DateFormatHelper;

impl HelperDef for DateFormatHelper {
    fn call_inner<'reg: 'rc, 'rc>(
        &self,
        helper: &Helper<'rc>,
        _: &'reg Handlebars<'reg>,
        _: &'rc Context,
        _: &mut RenderContext<'reg, 'rc>,
    ) -> Result<ScopedJson<'rc>, RenderError> {
        validate_arguments(helper, 2)?;
        let input = helper
            .param(0)
            .ok_or(RenderErrorReason::ParamNotFoundForIndex("date_format", 0))?
            .value();
        let format = helper
            .param(1)
            .ok_or(RenderErrorReason::ParamNotFoundForIndex("date_format", 1))?
            .value()
            .as_str()
            .ok_or_else(|| date_format_error("format must be a string"))?;
        let date = parse_date(input)?;
        let mut output = String::new();
        write!(&mut output, "{}", date.format(format))
            .map_err(|_| date_format_error("invalid strftime format"))?;
        Ok(ScopedJson::Derived(Value::String(output)))
    }
}

fn validate_arguments(helper: &Helper<'_>, expected: usize) -> Result<(), RenderError> {
    if helper.params().len() != expected || !helper.hash().is_empty() || helper.is_block() {
        return Err(RenderErrorReason::Other(format!(
            "{} expects {expected} positional arguments and no named arguments or block",
            helper.name()
        ))
        .into());
    }
    Ok(())
}

fn parse_date(input: &Value) -> Result<DateTime<Utc>, RenderError> {
    if let Some(value) = input.as_str() {
        return DateTime::parse_from_rfc3339(value)
            .map(|date| date.with_timezone(&Utc))
            .map_err(|_| date_format_error("date must be a valid RFC 3339 string"));
    }

    let timestamp = input.as_i64().ok_or_else(|| {
        date_format_error("date must be an RFC 3339 string or integer Unix seconds")
    })?;
    DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| date_format_error("Unix timestamp is out of range"))
}

fn date_format_error(message: &str) -> RenderError {
    RenderErrorReason::Other(format!("date_format: {message}")).into()
}
