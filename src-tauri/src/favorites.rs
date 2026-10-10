use chrono::{
    DateTime, Duration, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteItem {
    pub bvid: String,
    pub title: String,
    pub owner: String,
    pub duration: Option<f64>,
    pub cover_url: String,
    pub added_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteFolder {
    pub schema_version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub items: Vec<FavoriteItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    UnsupportedVersion,
    InvalidName,
    InvalidDescription,
    InvalidItems,
    InvalidBvid,
    InvalidTitle,
    InvalidOwner,
    InvalidCoverUrl,
    InvalidDuration,
    InvalidDate,
    InvalidImportFormat,
}

impl ValidationError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "不支持的收藏夹版本",
            Self::InvalidName => "名称无效",
            Self::InvalidDescription => "描述无效",
            Self::InvalidItems => "视频列表无效",
            Self::InvalidBvid => "BV 号无效",
            Self::InvalidTitle => "标题无效",
            Self::InvalidOwner => "UP 主无效",
            Self::InvalidCoverUrl => "封面地址无效",
            Self::InvalidDuration => "时长无效",
            Self::InvalidDate => "日期无效",
            Self::InvalidImportFormat => "导入格式或版本无效",
        }
    }
}

impl Display for ValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl Error for ValidationError {}

#[derive(Debug)]
pub enum ImportError {
    Json(serde_json::Error),
    Validation(ValidationError),
}

impl Display for ImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => Display::fmt(error, formatter),
            Self::Validation(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Validation(error) => Some(error),
        }
    }
}

impl From<serde_json::Error> for ImportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ValidationError> for ImportError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

pub fn parse_import(input: &str) -> Result<Vec<FavoriteFolder>, ImportError> {
    let value: Value = serde_json::from_str(input)?;
    Ok(validate_import(&value)?)
}

pub fn validate_import(data: &Value) -> Result<Vec<FavoriteFolder>, ValidationError> {
    let object = data.as_object();
    if !is_schema_version_one(field(object, "schemaVersion")) {
        return Err(ValidationError::InvalidImportFormat);
    }

    if field(object, "type").and_then(Value::as_str) == Some("bili-sound-favorites") {
        let folders = field(object, "folders")
            .and_then(Value::as_array)
            .ok_or(ValidationError::InvalidImportFormat)?;
        return folders.iter().map(validate_folder).collect();
    }

    Ok(vec![validate_folder(data)?])
}

pub fn validate_folder(folder: &Value) -> Result<FavoriteFolder, ValidationError> {
    let object = folder.as_object();
    if !is_schema_version_one(field(object, "schemaVersion")) {
        return Err(ValidationError::UnsupportedVersion);
    }

    let name = normalized_string(
        field(object, "name"),
        80,
        true,
        ValidationError::InvalidName,
    )?;
    let description = normalized_string(
        field(object, "description"),
        500,
        false,
        ValidationError::InvalidDescription,
    )?;
    let raw_items = field(object, "items")
        .and_then(Value::as_array)
        .ok_or(ValidationError::InvalidItems)?;

    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(raw_items.len());
    for raw_item in raw_items {
        let item = validate_item(raw_item)?;
        if seen.insert(item.bvid.clone()) {
            items.push(item);
        }
    }

    Ok(FavoriteFolder {
        schema_version: 1,
        id: field(object, "id").cloned(),
        name,
        description,
        created_at: validated_date(field(object, "createdAt"))?,
        updated_at: validated_date(field(object, "updatedAt"))?,
        items,
    })
}

fn validate_item(item: &Value) -> Result<FavoriteItem, ValidationError> {
    let object = item.as_object();
    let bvid = field(object, "bvid")
        .and_then(Value::as_str)
        .filter(|value| is_bvid(value))
        .ok_or(ValidationError::InvalidBvid)?
        .to_owned();
    let title = normalized_string(
        field(object, "title"),
        1000,
        true,
        ValidationError::InvalidTitle,
    )?;
    let owner = normalized_string(
        field(object, "owner"),
        500,
        false,
        ValidationError::InvalidOwner,
    )?;
    let cover_url = normalized_string(
        field(object, "coverUrl"),
        4096,
        false,
        ValidationError::InvalidCoverUrl,
    )?;
    if !cover_url.is_empty() {
        let parsed = Url::parse(&cover_url).map_err(|_| ValidationError::InvalidCoverUrl)?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(ValidationError::InvalidCoverUrl);
        }
    }

    let duration = match field(object, "duration") {
        Some(Value::Null) => None,
        Some(Value::Number(value)) => {
            let value = value.as_f64().ok_or(ValidationError::InvalidDuration)?;
            if !value.is_finite() || value < 0.0 {
                return Err(ValidationError::InvalidDuration);
            }
            Some(value)
        }
        _ => return Err(ValidationError::InvalidDuration),
    };

    Ok(FavoriteItem {
        bvid,
        title,
        owner,
        duration,
        cover_url,
        added_at: validated_date(field(object, "addedAt"))?,
    })
}

fn field<'a>(object: Option<&'a Map<String, Value>>, name: &str) -> Option<&'a Value> {
    object.and_then(|value| value.get(name))
}

fn is_schema_version_one(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64) == Some(1.0)
}

fn is_bvid(value: &str) -> bool {
    value.strip_prefix("BV").is_some_and(|suffix| {
        suffix.len() == 10 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn normalized_string(
    value: Option<&Value>,
    maximum_utf16_units: usize,
    required: bool,
    error: ValidationError,
) -> Result<String, ValidationError> {
    let value = value.and_then(Value::as_str).ok_or(error)?;
    if value.encode_utf16().count() > maximum_utf16_units {
        return Err(error);
    }
    let normalized = value.trim_matches(is_javascript_whitespace);
    if required && normalized.is_empty() {
        return Err(error);
    }
    Ok(normalized.to_owned())
}

fn is_javascript_whitespace(value: char) -> bool {
    matches!(
      value,
      '\u{0009}'..='\u{000d}'
        | '\u{0020}'
        | '\u{00a0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200a}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202f}'
        | '\u{205f}'
        | '\u{3000}'
        | '\u{feff}'
    )
}

fn validated_date(value: Option<&Value>) -> Result<String, ValidationError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or(ValidationError::InvalidDate)?;
    if !is_legacy_date(value) {
        return Err(ValidationError::InvalidDate);
    }
    Ok(value.to_owned())
}

fn is_legacy_date(value: &str) -> bool {
    parse_legacy_date(value).is_some()
}

struct LegacyDate {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
    offset_seconds: Option<i32>,
}

fn parse_legacy_date(value: &str) -> Option<LegacyDate> {
    let bytes = value.as_bytes();
    if bytes.len() < 16
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || bytes[7] != b'-'
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
        || bytes[10] != b'T'
        || !bytes[11..13].iter().all(u8::is_ascii_digit)
        || bytes[13] != b':'
        || !bytes[14..16].iter().all(u8::is_ascii_digit)
    {
        return None;
    }

    let year = four_digits(bytes, 0) as i32;
    let month = two_digits(bytes, 5) as u32;
    let day = two_digits(bytes, 8) as u32;
    let hour = two_digits(bytes, 11) as u32;
    let minute = two_digits(bytes, 14) as u32;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 24 || minute > 59 {
        return None;
    }

    let mut cursor = 16;
    let mut second = 0_u32;
    let mut has_seconds = false;
    if bytes.get(cursor) == Some(&b':') {
        if bytes.len() < cursor + 3 || !bytes[cursor + 1..cursor + 3].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        second = two_digits(bytes, cursor + 1) as u32;
        has_seconds = true;
        if second > 59 {
            return None;
        }
        cursor += 3;
    }

    let mut fraction_nonzero = false;
    let mut millisecond = 0_u32;
    if bytes.get(cursor) == Some(&b'.') {
        if !has_seconds {
            return None;
        }
        cursor += 1;
        let fraction_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            fraction_nonzero |= bytes[cursor] != b'0';
            if cursor < fraction_start + 3 {
                millisecond = millisecond * 10 + u32::from(bytes[cursor] - b'0');
            }
            cursor += 1;
        }
        if cursor == fraction_start {
            return None;
        }
        for _ in cursor - fraction_start..3 {
            millisecond *= 10;
        }
    }

    if hour == 24 && (minute != 0 || second != 0 || fraction_nonzero) {
        return None;
    }

    let offset_seconds = if cursor == bytes.len() {
        None
    } else if matches!(bytes[cursor], b'Z' | b'z') {
        if cursor + 1 != bytes.len() {
            return None;
        }
        Some(0)
    } else {
        let sign = match bytes[cursor] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let offset = &bytes[cursor + 1..];
        let (offset_hour, offset_minute) = match offset {
            [h1, h2, b':', m1, m2] if [h1, h2, m1, m2].into_iter().all(u8::is_ascii_digit) => (
                (h1 - b'0') * 10 + (h2 - b'0'),
                (m1 - b'0') * 10 + (m2 - b'0'),
            ),
            [h1, h2, m1, m2] if [h1, h2, m1, m2].into_iter().all(u8::is_ascii_digit) => (
                (h1 - b'0') * 10 + (h2 - b'0'),
                (m1 - b'0') * 10 + (m2 - b'0'),
            ),
            _ => return None,
        };
        if offset_hour > 23 || offset_minute > 59 {
            return None;
        }
        Some(sign * (i32::from(offset_hour) * 3600 + i32::from(offset_minute) * 60))
    };

    Some(LegacyDate {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millisecond,
        offset_seconds,
    })
}

fn two_digits(bytes: &[u8], start: usize) -> u8 {
    (bytes[start] - b'0') * 10 + (bytes[start + 1] - b'0')
}

fn four_digits(bytes: &[u8], start: usize) -> u16 {
    u16::from(two_digits(bytes, start)) * 100 + u16::from(two_digits(bytes, start + 2))
}

pub(crate) fn legacy_date_sort_key(value: &str) -> Option<i64> {
    let parsed = parse_legacy_date(value)?;
    let date = NaiveDate::from_ymd_opt(parsed.year, parsed.month, 1)?
        .checked_add_signed(Duration::days(i64::from(parsed.day - 1 + parsed.hour / 24)))?;
    let local = date.and_hms_milli_opt(
        parsed.hour % 24,
        parsed.minute,
        parsed.second,
        parsed.millisecond,
    )?;
    match parsed.offset_seconds {
        Some(0) => Some(Utc.from_utc_datetime(&local).timestamp_millis()),
        Some(seconds) => FixedOffset::east_opt(seconds)?
            .from_local_datetime(&local)
            .single()
            .map(|date| date.timestamp_millis()),
        None => local_timestamp_millis(local),
    }
}

fn local_timestamp_millis(local: NaiveDateTime) -> Option<i64> {
    match Local.from_local_datetime(&local) {
        LocalResult::Single(date) => Some(date.timestamp_millis()),
        LocalResult::Ambiguous(earlier, _) => Some(earlier.timestamp_millis()),
        LocalResult::None => {
            let before = nearest_valid_local(local, -1)?;
            let after = nearest_valid_local(local, 1)?;
            let adjusted = adjust_for_forward_gap(
                local,
                before.offset().local_minus_utc(),
                after.offset().local_minus_utc(),
            )?;
            match Local.from_local_datetime(&adjusted) {
                LocalResult::Single(date) => Some(date.timestamp_millis()),
                LocalResult::Ambiguous(earlier, _) => Some(earlier.timestamp_millis()),
                LocalResult::None => None,
            }
        }
    }
}

fn nearest_valid_local(local: NaiveDateTime, direction: i64) -> Option<DateTime<Local>> {
    const MAX_GAP_SEARCH_MINUTES: i64 = 2 * 24 * 60;
    (1..=MAX_GAP_SEARCH_MINUTES).find_map(|minutes| {
        let candidate = local.checked_add_signed(Duration::minutes(direction * minutes))?;
        match Local.from_local_datetime(&candidate) {
            LocalResult::Single(date) => Some(date),
            LocalResult::Ambiguous(earlier, _) => Some(earlier),
            LocalResult::None => None,
        }
    })
}

fn adjust_for_forward_gap(
    local: NaiveDateTime,
    before_offset_seconds: i32,
    after_offset_seconds: i32,
) -> Option<NaiveDateTime> {
    let gap_seconds = after_offset_seconds.checked_sub(before_offset_seconds)?;
    if gap_seconds <= 0 {
        return None;
    }
    local.checked_add_signed(Duration::seconds(i64::from(gap_seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VALID_SINGLE: &str =
        include_str!("../../referenceCODE/test/fixtures/favorites/valid-single-folder.json");
    const DUPLICATES: &str =
        include_str!("../../referenceCODE/test/fixtures/favorites/duplicates.json");
    const CORRUPT: &str =
        include_str!("../../referenceCODE/test/fixtures/favorites/corrupt-json.json");
    const MULTI_FOLDER: &str =
        include_str!("../../referenceCODE/test/fixtures/favorites/multi-folder.json");
    const PARTIAL_IMPORT: &str =
        include_str!("../../referenceCODE/test/fixtures/favorites/partial-import.json");

    fn valid_folder_value() -> Value {
        serde_json::from_str(VALID_SINGLE).expect("valid fixture")
    }

    fn assert_folder_error(folder: Value, expected: ValidationError) {
        assert_eq!(validate_folder(&folder), Err(expected));
        assert_eq!(expected.to_string(), expected.message());
    }

    #[test]
    fn favorites_valid_single_fixture_preserves_fields_and_items() {
        let folders = parse_import(VALID_SINGLE).expect("single folder fixture");
        assert_eq!(folders.len(), 1);
        let folder = &folders[0];
        assert_eq!(folder.schema_version, 1);
        assert_eq!(
            folder.id.as_ref().and_then(Value::as_str),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(folder.name, "Synthetic Folder Alpha");
        assert_eq!(folder.items.len(), 2);
        assert_eq!(folder.items[0].bvid, "BV1A2B3C4D5E");
        assert_eq!(folder.items[1].duration, Some(456.0));
    }

    #[test]
    fn favorites_duplicate_fixture_keeps_first_exact_bvid_and_case_variant() {
        let folder = &parse_import(DUPLICATES).expect("duplicate fixture")[0];
        assert_eq!(folder.items.len(), 2);
        assert_eq!(folder.items[0].bvid, "BV1D2U3P4L5M");
        assert_eq!(folder.items[0].title, "Synthetic First Occurrence");
        assert_eq!(folder.items[1].bvid, "BV1d2U3P4L5M");
    }

    #[test]
    fn favorites_corrupt_fixture_fails_json_deserialization() {
        assert!(matches!(parse_import(CORRUPT), Err(ImportError::Json(_))));
    }

    #[test]
    fn favorites_wrapped_fixtures_preserve_folder_and_item_order() {
        let multi = parse_import(MULTI_FOLDER).expect("multi-folder fixture");
        assert_eq!(multi.len(), 2);
        assert_eq!(multi[0].name, "Synthetic Collection One");
        assert_eq!(multi[1].name, "Synthetic Collection Two");
        assert_eq!(
            multi
                .iter()
                .map(|folder| folder.items.len())
                .collect::<Vec<_>>(),
            [2, 1]
        );

        let partial = parse_import(PARTIAL_IMPORT).expect("partial-import fixture");
        assert_eq!(partial.len(), 2);
        assert_eq!(partial[0].name, "Synthetic Partial One");
        assert_eq!(partial[1].name, "Synthetic Partial Two");
        assert_eq!(
            partial
                .iter()
                .map(|folder| folder.items.len())
                .collect::<Vec<_>>(),
            [1, 1]
        );
    }

    #[test]
    fn favorites_import_accepts_empty_wrapper_and_ignores_unknown_fields() {
        let empty = json!({
          "schemaVersion": 1,
          "type": "bili-sound-favorites",
          "folders": [],
          "padding": "ignored"
        });
        assert!(validate_import(&empty).expect("empty wrapper").is_empty());

        let mut folder = valid_folder_value();
        folder["unknown"] = json!("ignored");
        folder["items"][0]["unknown"] = json!(true);
        folder["id"] = json!("not-a-uuid");
        let normalized = validate_folder(&folder).expect("unknown fields ignored");
        let output = serde_json::to_value(normalized).expect("serialize normalized folder");
        assert_eq!(output["id"], "not-a-uuid");
        assert!(output.get("unknown").is_none());
        assert!(output["items"][0].get("unknown").is_none());

        let wrong_type = json!({
          "schemaVersion": 1,
          "type": "Bili-Sound-Favorites",
          "folders": []
        });
        assert_eq!(
            validate_import(&wrong_type),
            Err(ValidationError::InvalidName)
        );
    }

    #[test]
    fn favorites_import_format_errors_are_stable() {
        assert_eq!(
            validate_import(&json!({"schemaVersion": 2})),
            Err(ValidationError::InvalidImportFormat)
        );
        assert_eq!(
            validate_import(&json!({
              "schemaVersion": 1,
              "type": "bili-sound-favorites",
              "folders": {}
            })),
            Err(ValidationError::InvalidImportFormat)
        );
    }

    #[test]
    fn favorites_string_lengths_use_pre_trim_javascript_utf16_units() {
        let mut folder = valid_folder_value();
        folder["name"] = json!("a".repeat(80));
        assert_eq!(validate_folder(&folder).expect("80 units").name.len(), 80);
        folder["name"] = json!("a".repeat(81));
        assert_folder_error(folder.clone(), ValidationError::InvalidName);

        folder["name"] = json!(format!(" {}", "a".repeat(79)));
        assert_eq!(
            validate_folder(&folder).expect("pre-trim 80 units").name,
            "a".repeat(79)
        );
        folder["name"] = json!(format!(" {}", "a".repeat(80)));
        assert_folder_error(folder.clone(), ValidationError::InvalidName);

        folder["name"] = json!("😀".repeat(40));
        assert_eq!(
            validate_folder(&folder).expect("40 surrogate pairs").name,
            "😀".repeat(40)
        );
        folder["name"] = json!("😀".repeat(41));
        assert_folder_error(folder, ValidationError::InvalidName);
    }

    #[test]
    fn favorites_string_field_boundaries_and_normalization_match_legacy() {
        let mut folder = valid_folder_value();
        folder["name"] = json!(" \u{feff}名称 \u{00a0}");
        folder["description"] = json!("d".repeat(500));
        folder["items"][0]["title"] = json!(format!(" {} ", "t".repeat(998)));
        folder["items"][0]["owner"] = json!("o".repeat(500));
        let prefix = "https://example.com/";
        folder["items"][0]["coverUrl"] = json!(format!(
            "{prefix}{}",
            "c".repeat(4096 - prefix.encode_utf16().count())
        ));
        let normalized = validate_folder(&folder).expect("maximum lengths");
        assert_eq!(normalized.name, "名称");
        assert_eq!(normalized.items[0].title, "t".repeat(998));
        assert_eq!(normalized.items[0].cover_url.encode_utf16().count(), 4096);

        folder["description"] = json!("d".repeat(501));
        assert_folder_error(folder.clone(), ValidationError::InvalidDescription);
        folder["description"] = json!("");
        folder["items"][0]["title"] = json!("t".repeat(1001));
        assert_folder_error(folder.clone(), ValidationError::InvalidTitle);
        folder["items"][0]["title"] = json!("title");
        folder["items"][0]["owner"] = json!("o".repeat(501));
        assert_folder_error(folder.clone(), ValidationError::InvalidOwner);
        folder["items"][0]["owner"] = json!("");
        folder["items"][0]["coverUrl"] =
            json!(format!("{prefix}{}", "c".repeat(4097 - prefix.len())));
        assert_folder_error(folder, ValidationError::InvalidCoverUrl);
    }

    #[test]
    fn favorites_required_and_typed_fields_report_legacy_errors() {
        let mut folder = valid_folder_value();
        folder["schemaVersion"] = json!(2);
        assert_folder_error(folder.clone(), ValidationError::UnsupportedVersion);
        folder["schemaVersion"] = json!(1);
        folder["name"] = json!(" \t\n");
        assert_folder_error(folder.clone(), ValidationError::InvalidName);
        folder["name"] = json!("valid");
        folder["description"] = json!(null);
        assert_folder_error(folder.clone(), ValidationError::InvalidDescription);
        folder["description"] = json!("");
        folder["items"] = json!({});
        assert_folder_error(folder.clone(), ValidationError::InvalidItems);

        folder = valid_folder_value();
        folder["items"][0]["bvid"] = json!("bv1A2B3C4D5E");
        assert_folder_error(folder.clone(), ValidationError::InvalidBvid);
        folder["items"][0]["bvid"] = json!("BV1A2B3C4D5E");
        folder["items"][0]["title"] = json!("   ");
        assert_folder_error(folder.clone(), ValidationError::InvalidTitle);
        folder["items"][0]["title"] = json!("title");
        folder["items"][0]["owner"] = json!(false);
        assert_folder_error(folder.clone(), ValidationError::InvalidOwner);
        folder["items"][0]["owner"] = json!("");
        folder["items"][0]["coverUrl"] = json!("javascript:alert(1)");
        assert_folder_error(folder, ValidationError::InvalidCoverUrl);
    }

    #[test]
    fn favorites_duration_requires_explicit_null_or_nonnegative_finite_number() {
        for duration in [json!(null), json!(0), json!(42), json!(1.25)] {
            let mut folder = valid_folder_value();
            folder["items"][0]["duration"] = duration;
            validate_folder(&folder).expect("valid duration");
        }
        for duration in [json!(-1), json!("1"), json!(true)] {
            let mut folder = valid_folder_value();
            folder["items"][0]["duration"] = duration;
            assert_folder_error(folder, ValidationError::InvalidDuration);
        }
        let mut missing = valid_folder_value();
        missing["items"][0]
            .as_object_mut()
            .expect("item object")
            .remove("duration");
        assert_folder_error(missing, ValidationError::InvalidDuration);
    }

    #[test]
    fn favorites_dates_match_probed_legacy_boundaries_and_preserve_input() {
        let valid_dates = [
            "2026-01-02T03:04:05.000Z",
            "2026-01-02T03:04:05+08:00",
            "2026-01-02T03:04:05+0800",
            "2026-01-02T03:04:05",
            "2026-01-02T03:04",
            "2026-01-02T03:04:05.123456Z",
            "2026-01-02T03:04:05z",
            "2026-02-30T03:04:05Z",
            "2026-01-02T24:00:00Z",
        ];
        for date in valid_dates {
            let mut folder = valid_folder_value();
            folder["createdAt"] = json!(date);
            assert_eq!(
                validate_folder(&folder)
                    .expect("valid legacy date")
                    .created_at,
                date
            );
        }

        for date in [
            "2026-01-02 03:04:05Z",
            "2026-13-02T03:04:05Z",
            "2026-01-32T03:04:05Z",
            "2026-01-02T24:01:00Z",
            "2026-01-02T03:04.5Z",
            "2026-01-02T03:04.000+08:00",
            "not-a-date",
        ] {
            let mut folder = valid_folder_value();
            folder["createdAt"] = json!(date);
            assert_folder_error(folder, ValidationError::InvalidDate);
        }

        assert_eq!(
            legacy_date_sort_key("2026-01-02T03:04:05+0800"),
            legacy_date_sort_key("2026-01-02T03:04:05+08:00")
        );
        assert_eq!(
            legacy_date_sort_key("2026-01-02T03:04:05z"),
            legacy_date_sort_key("2026-01-02T03:04:05Z")
        );
        assert_eq!(
            legacy_date_sort_key("2026-01-02T03:04"),
            legacy_date_sort_key("2026-01-02T03:04:00")
        );
        assert!(
            legacy_date_sort_key("2026-01-02T03:05") > legacy_date_sort_key("2026-01-02T03:04")
        );
        assert_eq!(
            legacy_date_sort_key("2026-02-30T03:04:05Z"),
            legacy_date_sort_key("2026-03-02T03:04:05Z")
        );

        let spring_gap = NaiveDate::from_ymd_opt(2026, 3, 8)
            .expect("date")
            .and_hms_opt(2, 30, 0)
            .expect("time");
        assert_eq!(
            adjust_for_forward_gap(spring_gap, -5 * 3600, -4 * 3600),
            NaiveDate::from_ymd_opt(2026, 3, 8)
                .expect("date")
                .and_hms_opt(3, 30, 0)
        );
        assert_eq!(
            adjust_for_forward_gap(spring_gap, -4 * 3600, -5 * 3600),
            None
        );
        assert_eq!(
            adjust_for_forward_gap(spring_gap, 10 * 3600 + 30 * 60, 11 * 3600),
            NaiveDate::from_ymd_opt(2026, 3, 8)
                .expect("date")
                .and_hms_opt(3, 0, 0)
        );
    }

    #[test]
    fn favorites_duplicate_items_are_validated_before_filtering() {
        let mut folder = valid_folder_value();
        let duplicate = folder["items"][0].clone();
        let mut invalid_duplicate = duplicate;
        invalid_duplicate["title"] = json!("   ");
        folder["items"] = json!([folder["items"][0].clone(), invalid_duplicate]);
        assert_folder_error(folder, ValidationError::InvalidTitle);
    }
}
