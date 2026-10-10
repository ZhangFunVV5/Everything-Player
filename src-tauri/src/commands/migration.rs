use crate::favorites::{validate_import, FavoriteFolder, ValidationError};
use crate::repositories::favorites::{FavoritesRepository, RepositoryError};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

const MAX_JSON_BYTES: u64 = 10 * 1024 * 1024;
const RECORD_FILE_NAME: &str = "favorites-migration.json";
const STATUS_IMPORTABLE: &str = "importable";
const STATUS_CHANGED: &str = "changed";
const STATUS_MIGRATED: &str = "migrated";
const STATUS_PARTIAL: &str = "partial";
const STATUS_FAILED: &str = "failed";
const STATUS_INVALID: &str = "invalid";
const STATUS_UNSUPPORTED: &str = "unsupported";
const STATUS_READ_ERROR: &str = "readError";
const STATUS_TOO_LARGE: &str = "tooLarge";
const STATUS_EMPTY: &str = "empty";

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationFile {
    pub name: String,
    pub size: u64,
    pub status: String,
    pub folder_count: usize,
    pub target_folder_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationPreview {
    pub session_id: String,
    pub files: Vec<LegacyMigrationFile>,
    pub can_commit: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationResult {
    pub files: Vec<LegacyMigrationFile>,
    pub imported_folders: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_error: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct LegacyMigrationCommandError {
    pub message: String,
}

impl LegacyMigrationCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<RepositoryError> for LegacyMigrationCommandError {
    fn from(error: RepositoryError) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Default)]
pub struct LegacyMigrationState {
    sessions: Mutex<HashMap<String, LegacyMigrationSession>>,
    commit_lock: Mutex<()>,
}

#[derive(Debug)]
struct LegacyMigrationSession {
    files: Vec<LegacyMigrationSourceFile>,
}

#[derive(Debug)]
struct LegacyMigrationSourceFile {
    path: PathBuf,
    fingerprint: Option<String>,
    folders: Option<Vec<FavoriteFolder>>,
    preview: LegacyMigrationFile,
    processable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationRecords {
    version: u8,
    entries: Vec<MigrationRecord>,
}

impl Default for MigrationRecords {
    fn default() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationRecord {
    source_name: String,
    fingerprint: String,
    status: String,
    target_folder_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    completed_at: String,
}

#[derive(Debug)]
struct ReadSource {
    size: u64,
    fingerprint: String,
    folders: Vec<FavoriteFolder>,
}

#[derive(Debug)]
struct ReadIssue {
    status: &'static str,
    reason: String,
    size: u64,
}

#[tauri::command]
pub fn favorites_migration_select_legacy_directory(
    app: AppHandle,
    state: State<'_, LegacyMigrationState>,
) -> Result<Option<LegacyMigrationPreview>, LegacyMigrationCommandError> {
    let selected = app
        .dialog()
        .file()
        .set_title("选择旧收藏夹目录")
        .blocking_pick_folder();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let directory = selected
        .into_path()
        .map_err(|error| LegacyMigrationCommandError::new(error.to_string()))?;
    if !directory.is_absolute() {
        return Err(LegacyMigrationCommandError::new("旧收藏夹目录无效"));
    }

    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| LegacyMigrationCommandError::new(error.to_string()))?;
    let records = load_records(&app_data).map_err(LegacyMigrationCommandError::new)?;
    let session = scan_directory(&directory, &records).map_err(LegacyMigrationCommandError::new)?;
    let preview = preview_for_session(&session);
    let session_id = Uuid::new_v4().to_string();
    state
        .sessions
        .lock()
        .map_err(|_| LegacyMigrationCommandError::new("迁移会话不可用"))?
        .insert(session_id.clone(), session);

    Ok(Some(LegacyMigrationPreview {
        session_id,
        can_commit: can_commit_preview(&preview),
        files: preview,
    }))
}

#[tauri::command]
pub fn favorites_migration_cancel(
    session_id: String,
    state: State<'_, LegacyMigrationState>,
) -> Result<(), LegacyMigrationCommandError> {
    state
        .sessions
        .lock()
        .map_err(|_| LegacyMigrationCommandError::new("迁移会话不可用"))?
        .remove(&session_id);
    Ok(())
}

#[tauri::command]
pub fn favorites_migration_commit(
    session_id: String,
    app: AppHandle,
    state: State<'_, LegacyMigrationState>,
    repository: State<'_, FavoritesRepository>,
) -> Result<LegacyMigrationResult, LegacyMigrationCommandError> {
    let session = state
        .sessions
        .lock()
        .map_err(|_| LegacyMigrationCommandError::new("迁移会话不可用"))?
        .remove(&session_id)
        .ok_or_else(|| LegacyMigrationCommandError::new("迁移会话不存在"))?;
    let _commit_guard = state
        .commit_lock
        .lock()
        .map_err(|_| LegacyMigrationCommandError::new("迁移写入锁不可用"))?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| LegacyMigrationCommandError::new(error.to_string()))?;
    let mut records = load_records(&app_data).map_err(LegacyMigrationCommandError::new)?;
    Ok(commit_session(
        &repository,
        session,
        &app_data,
        &mut records,
    ))
}

fn scan_directory(
    directory: &Path,
    records: &MigrationRecords,
) -> Result<LegacyMigrationSession, String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|_| "无法读取旧收藏夹目录".to_owned())?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|_| "无法读取旧收藏夹目录".to_owned())?;
    entries.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut files = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        files.push(scan_source_file(path, name, records));
    }
    Ok(LegacyMigrationSession { files })
}

fn scan_source_file(
    path: PathBuf,
    name: String,
    records: &MigrationRecords,
) -> LegacyMigrationSourceFile {
    match read_source_file(&path) {
        Ok(source) => {
            let previous =
                records.entries.iter().rev().find(|entry| {
                    entry.source_name == name && entry.fingerprint == source.fingerprint
                });
            let source_was_seen = records
                .entries
                .iter()
                .any(|entry| entry.source_name == name);
            let (status, reason, target_folder_ids, processable) = match previous {
                Some(record) => (
                    record.status.clone(),
                    record.reason.clone(),
                    record.target_folder_ids.clone(),
                    record.status != STATUS_MIGRATED,
                ),
                None if source.folders.is_empty() => (
                    STATUS_EMPTY.to_owned(),
                    Some("没有可迁移的收藏夹".to_owned()),
                    Vec::new(),
                    false,
                ),
                None if source_was_seen => (
                    STATUS_CHANGED.to_owned(),
                    Some("源文件已变化，可作为新的迁移候选".to_owned()),
                    Vec::new(),
                    true,
                ),
                None => (STATUS_IMPORTABLE.to_owned(), None, Vec::new(), true),
            };
            LegacyMigrationSourceFile {
                path,
                fingerprint: Some(source.fingerprint),
                folders: Some(source.folders.clone()),
                preview: LegacyMigrationFile {
                    name,
                    size: source.size,
                    status,
                    folder_count: source.folders.len(),
                    target_folder_ids,
                    reason,
                },
                processable,
            }
        }
        Err(issue) => LegacyMigrationSourceFile {
            path,
            fingerprint: None,
            folders: None,
            preview: LegacyMigrationFile {
                name,
                size: issue.size,
                status: issue.status.to_owned(),
                folder_count: 0,
                target_folder_ids: Vec::new(),
                reason: Some(issue.reason),
            },
            processable: false,
        },
    }
}

fn read_source_file(path: &Path) -> Result<ReadSource, ReadIssue> {
    let metadata = fs::metadata(path).map_err(|_| ReadIssue {
        status: STATUS_READ_ERROR,
        reason: "读取失败".to_owned(),
        size: 0,
    })?;
    if !metadata.is_file() {
        return Err(ReadIssue {
            status: STATUS_READ_ERROR,
            reason: "读取失败".to_owned(),
            size: metadata.len(),
        });
    }
    if metadata.len() > MAX_JSON_BYTES {
        return Err(ReadIssue {
            status: STATUS_TOO_LARGE,
            reason: "文件不能超过 10 MB".to_owned(),
            size: metadata.len(),
        });
    }

    let mut file = File::open(path).map_err(|_| ReadIssue {
        status: STATUS_READ_ERROR,
        reason: "读取失败".to_owned(),
        size: metadata.len(),
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadIssue {
            status: STATUS_READ_ERROR,
            reason: "读取失败".to_owned(),
            size: metadata.len(),
        })?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(ReadIssue {
            status: STATUS_TOO_LARGE,
            reason: "文件不能超过 10 MB".to_owned(),
            size: bytes.len() as u64,
        });
    }

    let fingerprint = fingerprint(&bytes);
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| ReadIssue {
        status: STATUS_INVALID,
        reason: "文件不是有效 JSON".to_owned(),
        size: bytes.len() as u64,
    })?;
    let folders = validate_import(&data).map_err(|error| ReadIssue {
        status: match error {
            ValidationError::UnsupportedVersion | ValidationError::InvalidImportFormat => {
                STATUS_UNSUPPORTED
            }
            _ => STATUS_INVALID,
        },
        reason: error.to_string(),
        size: bytes.len() as u64,
    })?;
    Ok(ReadSource {
        size: bytes.len() as u64,
        fingerprint,
        folders,
    })
}

fn fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn preview_for_session(session: &LegacyMigrationSession) -> Vec<LegacyMigrationFile> {
    session
        .files
        .iter()
        .map(|file| file.preview.clone())
        .collect()
}

fn can_commit_preview(files: &[LegacyMigrationFile]) -> bool {
    files.iter().any(|file| {
        matches!(
            file.status.as_str(),
            STATUS_IMPORTABLE | STATUS_CHANGED | STATUS_PARTIAL | STATUS_FAILED
        )
    })
}

fn commit_session(
    repository: &FavoritesRepository,
    session: LegacyMigrationSession,
    app_data: &Path,
    records: &mut MigrationRecords,
) -> LegacyMigrationResult {
    let mut id_generator = || Uuid::new_v4().to_string();
    commit_session_with_id_generator(repository, session, app_data, records, &mut id_generator)
}

fn commit_session_with_id_generator<F>(
    repository: &FavoritesRepository,
    session: LegacyMigrationSession,
    app_data: &Path,
    records: &mut MigrationRecords,
    id_generator: &mut F,
) -> LegacyMigrationResult
where
    F: FnMut() -> String,
{
    let mut files = Vec::with_capacity(session.files.len());
    let mut record_dirty = false;
    let mut imported_folders = 0;

    for source in session.files {
        if !source.processable {
            files.push(source.preview);
            continue;
        }

        let Some(expected_fingerprint) = source.fingerprint.as_deref() else {
            files.push(source.preview);
            continue;
        };
        let current = match read_source_file(&source.path) {
            Ok(current) => current,
            Err(issue) => {
                files.push(LegacyMigrationFile {
                    name: source.preview.name,
                    size: issue.size,
                    status: issue.status.to_owned(),
                    folder_count: 0,
                    target_folder_ids: Vec::new(),
                    reason: Some(issue.reason),
                });
                continue;
            }
        };
        if current.fingerprint != expected_fingerprint {
            files.push(LegacyMigrationFile {
                name: source.preview.name,
                size: current.size,
                status: STATUS_CHANGED.to_owned(),
                folder_count: current.folders.len(),
                target_folder_ids: Vec::new(),
                reason: Some("源文件已变化，请重新预览".to_owned()),
            });
            continue;
        }
        let previous = records
            .entries
            .iter()
            .rev()
            .find(|entry| {
                entry.source_name == source.preview.name
                    && entry.fingerprint == expected_fingerprint
            })
            .cloned();
        if let Some(previous) = previous
            .as_ref()
            .filter(|entry| entry.status == STATUS_MIGRATED)
        {
            files.push(LegacyMigrationFile {
                name: source.preview.name,
                size: current.size,
                status: previous.status.clone(),
                folder_count: current.folders.len(),
                target_folder_ids: previous.target_folder_ids.clone(),
                reason: previous.reason.clone(),
            });
            continue;
        }

        let current_folder_count = current.folders.len();
        let mut target_folder_ids = previous
            .as_ref()
            .filter(|entry| entry.status == STATUS_PARTIAL)
            .map(|entry| entry.target_folder_ids.clone())
            .unwrap_or_default();
        let existing_target_folder_count = target_folder_ids.len();
        let mut failure = None;
        for mut folder in source
            .folders
            .unwrap_or(current.folders)
            .into_iter()
            .skip(target_folder_ids.len())
        {
            let target_id = id_generator();
            folder.id = Some(Value::String(target_id.clone()));
            match repository.save_new(folder) {
                Ok(_) => target_folder_ids.push(target_id),
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }

        let status = match (&failure, target_folder_ids.is_empty()) {
            (None, _) => STATUS_MIGRATED,
            (Some(_), true) => STATUS_FAILED,
            (Some(_), false) => STATUS_PARTIAL,
        };
        let reason = failure.map(|error| {
            if status == STATUS_PARTIAL {
                format!(
                    "已导入 {} 个收藏夹，后续写入失败：{error}",
                    target_folder_ids.len()
                )
            } else {
                error
            }
        });
        imported_folders += target_folder_ids.len() - existing_target_folder_count;
        record_dirty = true;
        records.entries.push(MigrationRecord {
            source_name: source.preview.name.clone(),
            fingerprint: expected_fingerprint.to_owned(),
            status: status.to_owned(),
            target_folder_ids: target_folder_ids.clone(),
            reason: reason.clone(),
            completed_at: now_iso_string(),
        });
        files.push(LegacyMigrationFile {
            name: source.preview.name,
            size: current.size,
            status: status.to_owned(),
            folder_count: current_folder_count,
            target_folder_ids,
            reason,
        });
    }

    let record_error = if record_dirty {
        store_records(app_data, records).err()
    } else {
        None
    };
    LegacyMigrationResult {
        files,
        imported_folders,
        record_error,
    }
}

fn migration_record_path(app_data: &Path) -> PathBuf {
    app_data.join(RECORD_FILE_NAME)
}

fn load_records(app_data: &Path) -> Result<MigrationRecords, String> {
    let path = migration_record_path(app_data);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MigrationRecords::default())
        }
        Err(_) => return Err("迁移记录读取失败".to_owned()),
    };
    let records: MigrationRecords =
        serde_json::from_str(&contents).map_err(|_| "迁移记录损坏".to_owned())?;
    if records.version != 1 {
        return Err("迁移记录版本不支持".to_owned());
    }
    Ok(records)
}

fn store_records(app_data: &Path, records: &MigrationRecords) -> Result<(), String> {
    let path = migration_record_path(app_data);
    let bytes = serde_json::to_vec_pretty(records).map_err(|_| "迁移记录序列化失败".to_owned())?;
    fs::create_dir_all(app_data).map_err(|_| "迁移记录写入失败".to_owned())?;
    let temporary = {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{}.tmp", Uuid::new_v4()));
        PathBuf::from(name)
    };
    let result = write_temporary_and_replace(&temporary, &path, &bytes)
        .map_err(|_| "迁移记录写入失败".to_owned());
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn write_temporary_and_replace(
    temporary: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), io::Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, destination)
}

fn now_iso_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VALID_SINGLE: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/valid-single-folder.json");
    const MULTI_FOLDER: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/multi-folder.json");

    struct TestDirectories {
        source: PathBuf,
        app_data: PathBuf,
    }

    impl TestDirectories {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "everything-player-legacy-migration-test-{}",
                Uuid::new_v4()
            ));
            let source = root.join("legacy-favo");
            let app_data = root.join("app-data");
            fs::create_dir_all(&source).expect("source directory");
            Self { source, app_data }
        }

        fn repository(&self) -> FavoritesRepository {
            FavoritesRepository::new(self.app_data.join("favorites"))
        }
    }

    impl Drop for TestDirectories {
        fn drop(&mut self) {
            if let Some(root) = self.source.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    fn write_fixture(path: &Path, fixture: &str) {
        fs::write(path, fixture).expect("fixture");
    }

    #[test]
    fn cancel_and_empty_directory_do_not_write_favorites_or_records() {
        let directories = TestDirectories::new();
        let records = MigrationRecords::default();
        let session = scan_directory(&directories.source, &records).expect("empty scan");
        assert!(session.files.is_empty());
        let result = commit_session(
            &directories.repository(),
            session,
            &directories.app_data,
            &mut MigrationRecords::default(),
        );
        assert_eq!(result.imported_folders, 0);
        assert!(!migration_record_path(&directories.app_data).exists());
        assert_eq!(
            directories.repository().list().expect("list").folders.len(),
            0
        );

        let state = LegacyMigrationState::default();
        state.sessions.lock().expect("lock").insert(
            "session".to_owned(),
            LegacyMigrationSession { files: Vec::new() },
        );
        state.sessions.lock().expect("lock").remove("session");
        assert!(state.sessions.lock().expect("lock").is_empty());
    }

    #[test]
    fn migrates_single_valid_file_with_new_id_and_atomic_record() {
        let directories = TestDirectories::new();
        let source_path = directories.source.join("legacy.json");
        write_fixture(&source_path, VALID_SINGLE);
        let mut records = MigrationRecords::default();
        let session = scan_directory(&directories.source, &records).expect("scan");
        let source_bytes = fs::read(&source_path).expect("source bytes");
        assert_eq!(session.files[0].preview.status, STATUS_IMPORTABLE);
        let result = commit_session(
            &directories.repository(),
            session,
            &directories.app_data,
            &mut records,
        );
        assert_eq!(result.imported_folders, 1);
        assert_eq!(result.files[0].status, STATUS_MIGRATED);
        assert_ne!(
            result.files[0].target_folder_ids[0],
            "11111111-1111-4111-8111-111111111111"
        );
        assert_eq!(
            fs::read(&source_path).expect("source unchanged"),
            source_bytes
        );
        assert_eq!(records.entries.len(), 1);
        let stored = directories.repository().list().expect("stored");
        assert_eq!(stored.folders.len(), 1);
        assert!(migration_record_path(&directories.app_data).is_file());
        assert!(fs::read_dir(&directories.app_data)
            .expect("record directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn mixed_files_report_statuses_and_keep_invalid_sources_unchanged() {
        let directories = TestDirectories::new();
        let valid = directories.source.join("valid.json");
        let corrupt = directories.source.join("corrupt.json");
        let unsupported = directories.source.join("unsupported.json");
        let oversized = directories.source.join("oversized.json");
        write_fixture(&valid, VALID_SINGLE);
        fs::write(&corrupt, b"{broken").expect("corrupt");
        fs::write(&unsupported, r#"{"schemaVersion":2,"name":"old"}"#).expect("unsupported");
        fs::write(&oversized, vec![b' '; (MAX_JSON_BYTES + 1) as usize]).expect("oversized");
        let corrupt_bytes = fs::read(&corrupt).expect("corrupt bytes");
        let unsupported_bytes = fs::read(&unsupported).expect("unsupported bytes");
        let oversized_bytes = fs::read(&oversized).expect("oversized bytes");

        let mut records = MigrationRecords::default();
        let session = scan_directory(&directories.source, &records).expect("scan");
        let statuses = session
            .files
            .iter()
            .map(|file| (file.preview.name.clone(), file.preview.status.clone()))
            .collect::<HashMap<_, _>>();
        assert_eq!(statuses["valid.json"], STATUS_IMPORTABLE);
        assert_eq!(statuses["corrupt.json"], STATUS_INVALID);
        assert_eq!(statuses["unsupported.json"], STATUS_UNSUPPORTED);
        assert_eq!(statuses["oversized.json"], STATUS_TOO_LARGE);
        let result = commit_session(
            &directories.repository(),
            session,
            &directories.app_data,
            &mut records,
        );
        assert_eq!(result.imported_folders, 1);
        assert_eq!(
            fs::read(&corrupt).expect("corrupt unchanged"),
            corrupt_bytes
        );
        assert_eq!(
            fs::read(&unsupported).expect("unsupported unchanged"),
            unsupported_bytes
        );
        assert_eq!(
            fs::read(&oversized).expect("oversized unchanged"),
            oversized_bytes
        );
    }

    #[test]
    fn repeated_source_is_migrated_once_and_changed_source_is_explicit() {
        let directories = TestDirectories::new();
        let source_path = directories.source.join("legacy.json");
        write_fixture(&source_path, VALID_SINGLE);
        let repository = directories.repository();
        let mut records = MigrationRecords::default();
        let first = commit_session(
            &repository,
            scan_directory(&directories.source, &records).expect("first scan"),
            &directories.app_data,
            &mut records,
        );
        assert_eq!(first.imported_folders, 1);
        records = load_records(&directories.app_data).expect("records");

        let repeated = scan_directory(&directories.source, &records).expect("repeat scan");
        assert_eq!(repeated.files[0].preview.status, STATUS_MIGRATED);
        let second = commit_session(&repository, repeated, &directories.app_data, &mut records);
        assert_eq!(second.imported_folders, 0);
        assert_eq!(repository.list().expect("one folder").folders.len(), 1);

        let mut changed_data: Value = serde_json::from_str(VALID_SINGLE).expect("json");
        changed_data["name"] = json!("Changed legacy folder");
        fs::write(
            &source_path,
            serde_json::to_vec_pretty(&changed_data).expect("changed json"),
        )
        .expect("changed source");
        let changed = scan_directory(&directories.source, &records).expect("changed scan");
        assert_eq!(changed.files[0].preview.status, STATUS_CHANGED);
        assert!(changed.files[0]
            .preview
            .reason
            .as_deref()
            .unwrap()
            .contains("变化"));
        let third = commit_session(&repository, changed, &directories.app_data, &mut records);
        assert_eq!(third.imported_folders, 1);
        assert_eq!(repository.list().expect("two folders").folders.len(), 2);
    }

    #[test]
    fn record_write_failure_keeps_imported_favorites_and_reports_record_error() {
        let directories = TestDirectories::new();
        write_fixture(&directories.source.join("legacy.json"), VALID_SINGLE);
        fs::create_dir_all(migration_record_path(&directories.app_data))
            .expect("blocking record path");
        let repository = directories.repository();
        let mut records = MigrationRecords::default();
        let result = commit_session(
            &repository,
            scan_directory(&directories.source, &records).expect("scan"),
            &directories.app_data,
            &mut records,
        );

        assert_eq!(result.imported_folders, 1);
        assert_eq!(result.files[0].status, STATUS_MIGRATED);
        assert_eq!(result.record_error.as_deref(), Some("迁移记录写入失败"));
        assert_eq!(
            repository
                .list()
                .expect("favorites remain writable")
                .folders
                .len(),
            1
        );
        assert!(migration_record_path(&directories.app_data).is_dir());
        assert!(fs::read_dir(&directories.app_data)
            .expect("app-data entries")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn failed_source_can_retry_all_folders_after_write_blocker_is_removed() {
        let directories = TestDirectories::new();
        write_fixture(&directories.source.join("legacy.json"), VALID_SINGLE);
        let repository = directories.repository();
        let blocked_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let favorites_directory = directories.app_data.join("favorites");
        fs::create_dir_all(&favorites_directory).expect("favorites directory");
        fs::create_dir(favorites_directory.join(format!("{blocked_id}.json")))
            .expect("blocking favorite path");

        let mut records = MigrationRecords::default();
        let mut first_id = || blocked_id.to_owned();
        let first = commit_session_with_id_generator(
            &repository,
            scan_directory(&directories.source, &records).expect("first scan"),
            &directories.app_data,
            &mut records,
            &mut first_id,
        );
        assert_eq!(first.imported_folders, 0);
        assert_eq!(first.files[0].status, STATUS_FAILED);
        assert!(first.files[0].target_folder_ids.is_empty());

        fs::remove_dir(favorites_directory.join(format!("{blocked_id}.json")))
            .expect("remove blocker");
        records = load_records(&directories.app_data).expect("failed record");
        let retry_preview = scan_directory(&directories.source, &records).expect("retry scan");
        assert_eq!(retry_preview.files[0].preview.status, STATUS_FAILED);
        assert!(retry_preview.files[0].processable);
        assert!(can_commit_preview(&preview_for_session(&retry_preview)));
        let mut retry_id = || blocked_id.to_owned();
        let retry = commit_session_with_id_generator(
            &repository,
            retry_preview,
            &directories.app_data,
            &mut records,
            &mut retry_id,
        );
        assert_eq!(retry.imported_folders, 1);
        assert_eq!(retry.files[0].status, STATUS_MIGRATED);
        assert_eq!(
            retry.files[0].target_folder_ids,
            vec![blocked_id.to_owned()]
        );
        assert_eq!(
            repository.list().expect("retried favorite").folders.len(),
            1
        );
        assert_eq!(
            records.entries.last().expect("migrated record").status,
            STATUS_MIGRATED
        );
    }

    #[test]
    fn partial_source_resumes_after_successful_prefix_without_duplicates() {
        let directories = TestDirectories::new();
        write_fixture(&directories.source.join("legacy.json"), MULTI_FOLDER);
        let repository = directories.repository();
        let first_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let second_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
        let favorites_directory = directories.app_data.join("favorites");
        fs::create_dir_all(&favorites_directory).expect("favorites directory");
        fs::create_dir(favorites_directory.join(format!("{second_id}.json")))
            .expect("blocking second favorite path");

        let mut records = MigrationRecords::default();
        let mut first_attempt_ids = vec![first_id, second_id].into_iter();
        let mut first_attempt_id = || {
            first_attempt_ids
                .next()
                .expect("first attempt id")
                .to_owned()
        };
        let first = commit_session_with_id_generator(
            &repository,
            scan_directory(&directories.source, &records).expect("first scan"),
            &directories.app_data,
            &mut records,
            &mut first_attempt_id,
        );
        assert_eq!(first.imported_folders, 1);
        assert_eq!(first.files[0].status, STATUS_PARTIAL);
        assert_eq!(first.files[0].target_folder_ids, vec![first_id.to_owned()]);
        assert_eq!(repository.list().expect("prefix favorite").folders.len(), 1);

        fs::remove_dir(favorites_directory.join(format!("{second_id}.json")))
            .expect("remove second blocker");
        records = load_records(&directories.app_data).expect("partial record");
        let retry_preview = scan_directory(&directories.source, &records).expect("retry scan");
        assert_eq!(retry_preview.files[0].preview.status, STATUS_PARTIAL);
        assert_eq!(
            retry_preview.files[0].preview.target_folder_ids,
            vec![first_id.to_owned()]
        );
        assert!(retry_preview.files[0].processable);
        assert!(can_commit_preview(&preview_for_session(&retry_preview)));
        let mut retry_id = || second_id.to_owned();
        let retry = commit_session_with_id_generator(
            &repository,
            retry_preview,
            &directories.app_data,
            &mut records,
            &mut retry_id,
        );
        assert_eq!(retry.imported_folders, 1);
        assert_eq!(retry.files[0].status, STATUS_MIGRATED);
        assert_eq!(
            retry.files[0].target_folder_ids,
            vec![first_id.to_owned(), second_id.to_owned()]
        );
        assert_eq!(
            repository
                .list()
                .expect("two resumed favorites")
                .folders
                .len(),
            2
        );
        let migrated = records.entries.last().expect("migrated record");
        assert_eq!(migrated.status, STATUS_MIGRATED);
        assert_eq!(
            migrated.target_folder_ids,
            vec![first_id.to_owned(), second_id.to_owned()]
        );

        let repeated = scan_directory(&directories.source, &records).expect("repeat scan");
        assert_eq!(repeated.files[0].preview.status, STATUS_MIGRATED);
        let repeated_result =
            commit_session(&repository, repeated, &directories.app_data, &mut records);
        assert_eq!(repeated_result.imported_folders, 0);
        assert_eq!(
            repository
                .list()
                .expect("idempotent favorites")
                .folders
                .len(),
            2
        );
    }

    #[test]
    fn multiple_valid_files_keep_source_names_and_get_distinct_ids() {
        let directories = TestDirectories::new();
        write_fixture(&directories.source.join("a.json"), VALID_SINGLE);
        write_fixture(&directories.source.join("b.json"), MULTI_FOLDER);
        let repository = directories.repository();
        let mut records = MigrationRecords::default();
        let result = commit_session(
            &repository,
            scan_directory(&directories.source, &records).expect("scan"),
            &directories.app_data,
            &mut records,
        );
        assert_eq!(result.imported_folders, 3);
        assert_eq!(result.files.len(), 2);
        assert_ne!(
            result.files[0].target_folder_ids[0],
            result.files[1].target_folder_ids[0]
        );
        assert_eq!(repository.list().expect("folders").folders.len(), 3);
    }

    #[test]
    fn preview_does_not_expose_source_directory() {
        let directories = TestDirectories::new();
        write_fixture(&directories.source.join("legacy.json"), VALID_SINGLE);
        let session =
            scan_directory(&directories.source, &MigrationRecords::default()).expect("scan");
        let preview = LegacyMigrationPreview {
            session_id: "opaque-session".to_owned(),
            can_commit: true,
            files: preview_for_session(&session),
        };
        let serialized = serde_json::to_string(&preview).expect("preview json");
        assert!(!serialized.contains(&directories.source.to_string_lossy().to_string()));
        assert!(!serialized.contains("legacy-favo"));
    }
}
