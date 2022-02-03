use super::WMIError;
use serde::{de, ser};
use std::fmt;
use std::str::FromStr;

#[cfg(all(not(feature = "time-instead-of-chrono"), not(feature = "default")))]
std::compile_error!("wmi::datetime::WMIDateTime must be available: either use the 'default' or 'time-instead-of-chrono' feature");

#[cfg(not(feature = "time-instead-of-chrono"))]
use chrono::prelude::*;

#[cfg(feature = "time-instead-of-chrono")]
use time::{
    format_description::{well_known::Rfc3339, FormatItem},
    macros::format_description,
    parsing::Parsed,
    PrimitiveDateTime, UtcOffset,
};

/// A wrapper type around `chrono`'s `DateTime` (`time`'s `OffsetDateTime` if the
// `time-instead-of-chrono` feature is active), which supports parsing from WMI-format strings.
#[derive(Debug)]
pub struct WMIDateTime(
    #[cfg(not(feature = "time-instead-of-chrono"))] pub chrono::DateTime<FixedOffset>,
    #[cfg(feature = "time-instead-of-chrono")] pub time::OffsetDateTime,
);

impl FromStr for WMIDateTime {
    type Err = WMIError;

    #[cfg(not(feature = "time-instead-of-chrono"))]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < 21 {
            return Err(WMIError::ConvertDatetimeError(s.into()));
        }

        let (datetime_part, tz_part) = s.split_at(21);
        let tz_min: i32 = tz_part.parse()?;
        let tz = FixedOffset::east(tz_min * 60);
        let dt = tz.datetime_from_str(datetime_part, "%Y%m%d%H%M%S.%f")?;

        Ok(Self(dt))
    }

    #[cfg(feature = "time-instead-of-chrono")]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() < 21 {
            return Err(WMIError::ConvertDatetimeError(s.into()));
        }

        // We have to ignore the year here, see bottom of https://time-rs.github.io/book/api/format-description.html
        // about the large-dates feature (permanent link:
        // https://github.com/time-rs/book/blob/0476c5bb35b512ac0cbda5c6cd5f0d0628b0269e/src/api/format-description.md?plain=1#L205)
        const TIME_FORMAT: &[FormatItem<'static>] =
            format_description!("[month][day][hour][minute][second].[subsecond digits:6]");

        let minutes_offset = s[21..].parse::<i32>()?;
        let offset =
            UtcOffset::from_whole_seconds(minutes_offset * 60).map_err(time::Error::from)?;

        let mut parser = Parsed::new();

        let naive_date_time = &s[4..21];
        parser
            .parse_items(naive_date_time.as_bytes(), TIME_FORMAT)
            .map_err(time::Error::from)?;
        // Microsoft thinks it is okay to return a subsecond value in microseconds but not put the zeros before it
        // so 1.1 is 1 second and 100 microsecond, ergo 1.000100 ...
        parser
            .set_subsecond(parser.subsecond().unwrap_or(0) / 1000)
            .ok_or_else(|| {
                WMIError::ParseDatetimeError(
                    time::error::Format::InvalidComponent("subsecond").into(),
                )
            })?;

        let naive_year = s[..4].parse::<i32>()?;
        parser.set_year(naive_year).ok_or_else(|| {
            WMIError::ParseDatetimeError(time::error::Format::InvalidComponent("year").into())
        })?;

        let naive_date_time: PrimitiveDateTime =
            std::convert::TryInto::try_into(parser).map_err(time::Error::from)?;
        let dt = naive_date_time.assume_offset(offset);
        Ok(Self(dt))
    }
}

struct DateTimeVisitor;

impl<'de> de::Visitor<'de> for DateTimeVisitor {
    type Value = WMIDateTime;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a timestamp in WMI format")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        value.parse().map_err(|err| E::custom(format!("{}", err)))
    }
}

impl<'de> de::Deserialize<'de> for WMIDateTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_str(DateTimeVisitor)
    }
}

impl ser::Serialize for WMIDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        #[cfg(not(feature = "time-instead-of-chrono"))]
        let formatted = self.0.to_rfc3339();
        // Unwrap: we passed a well known format, if it fails something has gone very wrong
        #[cfg(feature = "time-instead-of-chrono")]
        let formatted = self.0.format(&Rfc3339).unwrap();

        serializer.serialize_str(&formatted)
    }
}

#[cfg(test)]
mod tests {
    use super::WMIDateTime;
    use serde_json;
    #[cfg(feature = "time-instead-of-chrono")]
    use time::format_description::well_known::Rfc3339;

    #[test]
    fn it_works_with_negative_offset() {
        let dt: WMIDateTime = "20190113200517.500000-180".parse().unwrap();

        #[cfg(not(feature = "time-instead-of-chrono"))]
        let formatted = dt.0.to_rfc3339();
        #[cfg(feature = "time-instead-of-chrono")]
        let formatted = dt.0.format(&Rfc3339).unwrap();

        assert_eq!(formatted, "2019-01-13T20:05:17.000500-03:00");
    }

    #[test]
    fn it_works_with_positive_offset() {
        let dt: WMIDateTime = "20190113200517.500000+060".parse().unwrap();

        #[cfg(not(feature = "time-instead-of-chrono"))]
        let formatted = dt.0.to_rfc3339();
        #[cfg(feature = "time-instead-of-chrono")]
        let formatted = dt.0.format(&Rfc3339).unwrap();

        assert_eq!(formatted, "2019-01-13T20:05:17.000500+01:00");
    }

    #[test]
    fn it_fails_with_malformed_str() {
        let dt_res: Result<WMIDateTime, _> = "20190113200517".parse();

        assert!(dt_res.is_err());
    }

    #[test]
    fn it_fails_with_malformed_str_with_no_tz() {
        let dt_res: Result<WMIDateTime, _> = "20190113200517.000500".parse();

        assert!(dt_res.is_err());
    }

    #[test]
    fn it_serializes_to_rfc() {
        let dt: WMIDateTime = "20190113200517.500000+060".parse().unwrap();

        let v = serde_json::to_string(&dt).unwrap();
        assert_eq!(v, "\"2019-01-13T20:05:17.000500+01:00\"");
    }
}
