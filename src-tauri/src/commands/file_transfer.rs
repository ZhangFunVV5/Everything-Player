use crate::favorites::validate_import;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

const MAX_JSON_BYTES: u64 = 10 * 1024 * 1024;
const INVALID_JSON: &str = "文件不是有效 JSON";
const TOO_LARGE: &str = "文件不能超过 10 MB";
const DEFAULT_SUGGESTED_NAME: &str = "favorites.json";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JsonFileSelection {
    pub name: String,
    pub size: u64,
    pub data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonFileSaveRequest {
    pub suggested_name: String,
    pub data: Value,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FileTransferCommandError {
    pub message: String,
}

impl FileTransferCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[tauri::command]
pub async fn favorites_select_json_file(
    app: AppHandle,
) -> Result<Option<JsonFileSelection>, FileTransferCommandError> {
    let selected = app
        .dialog()
        .file()
        .set_title("导入收藏夹")
        .add_filter("JSON 文件", &["json"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|error| FileTransferCommandError::new(error.to_string()))?;
    read_json_file(&path)
        .map(Some)
        .map_err(FileTransferCommandError::new)
}

#[tauri::command]
pub async fn favorites_save_json_file(
    app: AppHandle,
    request: JsonFileSaveRequest,
) -> Result<(), FileTransferCommandError> {
    let content = serialize_export_data(&request.data)?;
    let suggested_name = sanitize_suggested_name(&request.suggested_name);
    let selected = app
        .dialog()
        .file()
        .set_title("导出收藏夹")
        .set_file_name(suggested_name)
        .add_filter("JSON 文件", &["json"])
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(());
    };
    let path = selected
        .into_path()
        .map_err(|error| FileTransferCommandError::new(error.to_string()))?;
    write_json_file(&path, &content)
}

fn read_json_file(path: &Path) -> Result<JsonFileSelection, &'static str> {
    let metadata = fs::metadata(path).map_err(|_| INVALID_JSON)?;
    if metadata.len() > MAX_JSON_BYTES {
        return Err(TOO_LARGE);
    }

    let mut file = File::open(path).map_err(|_| INVALID_JSON)?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_JSON_BYTES) as usize);
    file.by_ref()
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID_JSON)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(TOO_LARGE);
    }

    let text = String::from_utf8(bytes).map_err(|_| INVALID_JSON)?;
    let size = text.len() as u64;
    let data = serde_json::from_str(&text).map_err(|_| INVALID_JSON)?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or(INVALID_JSON)?;
    Ok(JsonFileSelection { name, size, data })
}

fn write_json_file(path: &Path, content: &str) -> Result<(), FileTransferCommandError> {
    fs::write(path, content).map_err(|error| FileTransferCommandError::new(error.to_string()))
}

fn serialize_export_data(data: &Value) -> Result<String, FileTransferCommandError> {
    let folders =
        validate_import(data).map_err(|error| FileTransferCommandError::new(error.to_string()))?;
    let output = if data.get("type").and_then(Value::as_str) == Some("bili-sound-favorites") {
        serde_json::json!({
            "schemaVersion": 1,
            "type": "bili-sound-favorites",
            "folders": folders,
        })
    } else {
        serde_json::to_value(
            folders
                .into_iter()
                .next()
                .ok_or_else(|| FileTransferCommandError::new("导入格式或版本无效"))?,
        )
        .map_err(|error| FileTransferCommandError::new(error.to_string()))?
    };
    serde_json::to_string_pretty(&output)
        .map_err(|error| FileTransferCommandError::new(error.to_string()))
}

fn sanitize_suggested_name(value: &str) -> String {
    let candidate = value
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or_default()
        .trim();
    if candidate.is_empty()
        || candidate == "."
        || candidate == ".."
        || candidate.chars().any(char::is_control)
    {
        DEFAULT_SUGGESTED_NAME.to_owned()
    } else {
        candidate.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    const VALID_SINGLE: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/valid-single-folder.json");

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "everything-player-file-transfer-test-{}",
                Uuid::new_v4()
            )))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_valid_json_and_preserves_name_and_size() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).expect("directory");
        let path = directory.0.join("favorites.json");
        fs::write(&path, VALID_SINGLE).expect("fixture");

        let selected = read_json_file(&path).expect("selected");
        assert_eq!(selected.name, "favorites.json");
        assert_eq!(selected.size as usize, VALID_SINGLE.len());
        assert_eq!(selected.data["schemaVersion"], json!(1));
    }

    #[test]
    fn rejects_invalid_utf8_and_json_with_legacy_message() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).expect("directory");
        let invalid_utf8 = directory.0.join("invalid-utf8.json");
        fs::write(&invalid_utf8, [0xff, 0xfe]).expect("invalid utf8");
        assert_eq!(read_json_file(&invalid_utf8), Err(INVALID_JSON));

        let invalid_json = directory.0.join("invalid-json.json");
        fs::write(&invalid_json, b"{broken").expect("invalid json");
        assert_eq!(read_json_file(&invalid_json), Err(INVALID_JSON));
    }

    #[test]
    fn rejects_over_limit_before_json_parsing_and_accepts_exact_limit() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).expect("directory");

        let exact = directory.0.join("exact.json");
        let exact_content = format!("null{}", " ".repeat(MAX_JSON_BYTES as usize - 4));
        fs::write(&exact, &exact_content).expect("exact limit");
        let selected = read_json_file(&exact).expect("exact limit accepted");
        assert_eq!(selected.size, MAX_JSON_BYTES);

        let over = directory.0.join("over.json");
        fs::write(&over, format!("{}x", exact_content)).expect("over limit");
        assert_eq!(read_json_file(&over), Err(TOO_LARGE));
    }

    #[test]
    fn validates_and_pretty_prints_single_and_bundle_exports() {
        let single: Value = serde_json::from_str(VALID_SINGLE).expect("single fixture");
        let single_json = serialize_export_data(&single).expect("single export");
        assert!(single_json.contains("\n  \"schemaVersion\": 1"));

        let bundle = json!({
            "schemaVersion": 1,
            "type": "bili-sound-favorites",
            "folders": [single]
        });
        let bundle_json = serialize_export_data(&bundle).expect("bundle export");
        let parsed_bundle = serde_json::from_str::<Value>(&bundle_json).expect("bundle json");
        assert_eq!(parsed_bundle["schemaVersion"], json!(1));
        assert_eq!(parsed_bundle["type"], json!("bili-sound-favorites"));
        assert_eq!(parsed_bundle["folders"][0]["id"], single["id"]);
        assert_eq!(parsed_bundle["folders"][0]["name"], single["name"]);
        assert_eq!(
            serialize_export_data(&json!({ "schemaVersion": 2 })),
            Err(FileTransferCommandError::new("导入格式或版本无效"))
        );
    }

    #[test]
    fn writes_json_and_reports_write_failures() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).expect("directory");
        let path = directory.0.join("saved.json");
        write_json_file(&path, "{\n  \"schemaVersion\": 1\n}").expect("write");
        assert_eq!(
            fs::read_to_string(&path).expect("saved content"),
            "{\n  \"schemaVersion\": 1\n}"
        );

        let failure = write_json_file(&directory.0, "{}").expect_err("directory write");
        assert!(!failure.message.is_empty());
    }

    #[test]
    fn suggested_name_is_always_a_safe_basename() {
        assert_eq!(
            sanitize_suggested_name("C:\\temp\\favorites.json"),
            "favorites.json"
        );
        assert_eq!(
            sanitize_suggested_name("../favorites.json"),
            "favorites.json"
        );
        assert_eq!(sanitize_suggested_name(".."), DEFAULT_SUGGESTED_NAME);
        assert_eq!(sanitize_suggested_name(""), DEFAULT_SUGGESTED_NAME);
    }
}
