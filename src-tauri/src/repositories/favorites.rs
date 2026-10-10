use crate::favorites::{
    legacy_date_sort_key, validate_folder, FavoriteFolder, FavoriteItem, ValidationError,
};
use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct FavoriteMetadata {
    pub bvid: String,
    pub title: String,
    pub owner: String,
    pub duration: Option<f64>,
    pub cover_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddItemResult {
    pub folder: FavoriteFolder,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListResult {
    pub folders: Vec<FavoriteFolder>,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum RepositoryError {
    InvalidId,
    NotFound,
    AlreadyExists,
    Validation(ValidationError),
    Json(serde_json::Error),
    Io(io::Error),
    AppData(String),
    Lock,
}

impl Display for RepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId => formatter.write_str("收藏夹 ID 无效"),
            Self::NotFound => formatter.write_str("收藏夹不存在"),
            Self::AlreadyExists => formatter.write_str("收藏夹已存在"),
            Self::Validation(error) => Display::fmt(error, formatter),
            Self::Json(error) => Display::fmt(error, formatter),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::AppData(error) => formatter.write_str(error),
            Self::Lock => formatter.write_str("收藏夹写入锁不可用"),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ValidationError> for RepositoryError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

impl From<serde_json::Error> for RepositoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<io::Error> for RepositoryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub struct FavoritesRepository {
    directory: PathBuf,
    folder_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl FavoritesRepository {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            folder_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Self, RepositoryError> {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|error| RepositoryError::AppData(error.to_string()))?;
        Ok(Self::new(app_data.join("favorites")))
    }

    pub fn list(&self) -> Result<ListResult, RepositoryError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ListResult {
                    folders: Vec::new(),
                    errors: Vec::new(),
                });
            }
            Err(error) => return Err(error.into()),
        };

        let mut folders = Vec::new();
        let mut errors = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = file_name.strip_suffix(".json") else {
                continue;
            };
            match self.get(id) {
                Ok(folder) => folders.push(folder),
                Err(error) => errors.push(format!("{file_name}：{error}")),
            }
        }

        folders.sort_by(|left, right| {
            legacy_date_sort_key(&right.updated_at).cmp(&legacy_date_sort_key(&left.updated_at))
        });
        Ok(ListResult { folders, errors })
    }

    pub fn get(&self, id: &str) -> Result<FavoriteFolder, RepositoryError> {
        let path = self.path(id)?;
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(RepositoryError::NotFound);
            }
            Err(error) => return Err(error.into()),
        };
        let value: Value = serde_json::from_str(&contents)?;
        let mut folder = validate_folder(&value)?;
        folder.id = Some(Value::String(id.to_owned()));
        Ok(folder)
    }

    pub fn save(&self, folder: FavoriteFolder) -> Result<FavoriteFolder, RepositoryError> {
        let id = folder_id(&folder)?.to_owned();
        let folder_lock = self.folder_lock(&id)?;
        let _guard = folder_lock.lock().map_err(|_| RepositoryError::Lock)?;
        self.write_validated(folder)
    }

    pub fn save_new(&self, folder: FavoriteFolder) -> Result<FavoriteFolder, RepositoryError> {
        let id = folder_id(&folder)?.to_owned();
        let folder_lock = self.folder_lock(&id)?;
        let _guard = folder_lock.lock().map_err(|_| RepositoryError::Lock)?;
        let value = serde_json::to_value(folder)?;
        let folder = validate_folder(&value)?;
        let path = self.path(&id)?;
        if path.exists() {
            return Err(RepositoryError::AlreadyExists);
        }
        let bytes = serde_json::to_vec_pretty(&folder)?;
        fs::create_dir_all(&self.directory)?;
        if path.exists() {
            return Err(RepositoryError::AlreadyExists);
        }
        let temporary = temporary_path(&path);
        let write_result = write_temporary_and_replace(&temporary, &path, &bytes);
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;
        Ok(folder)
    }

    pub fn create(&self, name: &str, description: &str) -> Result<FavoriteFolder, RepositoryError> {
        let now = now_iso_string();
        self.save(FavoriteFolder {
            schema_version: 1,
            id: Some(Value::String(Uuid::new_v4().to_string())),
            name: name.to_owned(),
            description: description.to_owned(),
            created_at: now.clone(),
            updated_at: now,
            items: Vec::new(),
        })
    }

    pub fn add_item(
        &self,
        id: &str,
        metadata: FavoriteMetadata,
    ) -> Result<AddItemResult, RepositoryError> {
        let folder_lock = self.folder_lock(id)?;
        let _guard = folder_lock.lock().map_err(|_| RepositoryError::Lock)?;
        let mut folder = self.get(id)?;
        if folder.items.iter().any(|item| item.bvid == metadata.bvid) {
            return Ok(AddItemResult {
                folder,
                duplicate: true,
            });
        }

        let now = now_iso_string();
        folder.items.insert(
            0,
            FavoriteItem {
                bvid: metadata.bvid,
                title: metadata.title,
                owner: metadata.owner,
                duration: metadata.duration,
                cover_url: metadata.cover_url,
                added_at: now.clone(),
            },
        );
        folder.updated_at = now;
        let folder = self.write_validated(folder)?;
        Ok(AddItemResult {
            folder,
            duplicate: false,
        })
    }

    fn path(&self, id: &str) -> Result<PathBuf, RepositoryError> {
        if !is_legacy_folder_id(id) {
            return Err(RepositoryError::InvalidId);
        }
        Ok(self.directory.join(format!("{id}.json")))
    }

    fn folder_lock(&self, id: &str) -> Result<Arc<Mutex<()>>, RepositoryError> {
        self.path(id)?;
        let mut locks = self
            .folder_locks
            .lock()
            .map_err(|_| RepositoryError::Lock)?;
        Ok(Arc::clone(
            locks
                .entry(id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }

    fn write_validated(&self, folder: FavoriteFolder) -> Result<FavoriteFolder, RepositoryError> {
        let value = serde_json::to_value(folder)?;
        let folder = validate_folder(&value)?;
        let id = folder_id(&folder)?.to_owned();
        let path = self.path(&id)?;
        let bytes = serde_json::to_vec_pretty(&folder)?;
        fs::create_dir_all(&self.directory)?;

        let temporary = temporary_path(&path);
        let write_result = write_temporary_and_replace(&temporary, &path, &bytes);
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;
        Ok(folder)
    }
}

fn folder_id(folder: &FavoriteFolder) -> Result<&str, RepositoryError> {
    folder
        .id
        .as_ref()
        .and_then(Value::as_str)
        .filter(|id| is_legacy_folder_id(id))
        .ok_or(RepositoryError::InvalidId)
}

fn is_legacy_folder_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes[..8].iter().all(u8::is_ascii_hexdigit)
        && bytes[8] == b'-'
        && bytes[9..]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{}.tmp", Uuid::new_v4()));
    PathBuf::from(name)
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
    use crate::favorites::parse_import;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

    const VALID_SINGLE: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/valid-single-folder.json");

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "everything-player-favorites-test-{}",
                Uuid::new_v4()
            ));
            Self(path)
        }

        fn repository(&self) -> FavoritesRepository {
            FavoritesRepository::new(self.0.clone())
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_folder(id: &str) -> FavoriteFolder {
        let mut folder = parse_import(VALID_SINGLE).expect("fixture").remove(0);
        folder.id = Some(Value::String(id.to_owned()));
        folder
    }

    fn metadata(index: usize) -> FavoriteMetadata {
        FavoriteMetadata {
            bvid: format!("BV1A2B3C4D5{:X}", index % 16),
            title: format!("Synthetic {index}"),
            owner: "Synthetic Owner".to_owned(),
            duration: Some(index as f64),
            cover_url: "https://example.com/cover.jpg".to_owned(),
        }
    }

    #[test]
    fn favorites_repository_empty_create_get_save_and_restart() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        assert_eq!(
            repository.list().expect("empty list"),
            ListResult {
                folders: vec![],
                errors: vec![]
            }
        );

        let created = repository
            .create("  Synthetic Folder  ", "  Description  ")
            .expect("create");
        let id = folder_id(&created).expect("id").to_owned();
        assert_eq!(created.name, "Synthetic Folder");
        assert_eq!(created.description, "Description");
        assert!(directory.0.join(format!("{id}.json")).is_file());

        let mut changed = repository.get(&id).expect("get");
        changed.description = "Changed".to_owned();
        repository.save(changed).expect("replace existing file");

        let restarted = directory.repository();
        assert_eq!(
            restarted.get(&id).expect("restart get").description,
            "Changed"
        );
    }

    #[test]
    fn favorites_repository_add_is_newest_first_and_duplicate_is_no_op() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let created = repository.create("Folder", "").expect("create");
        let id = folder_id(&created).expect("id").to_owned();

        let first = repository.add_item(&id, metadata(1)).expect("first add");
        assert!(!first.duplicate);
        let before_duplicate = fs::read(directory.0.join(format!("{id}.json"))).expect("before");
        let duplicate = repository.add_item(&id, metadata(1)).expect("duplicate");
        let after_duplicate = fs::read(directory.0.join(format!("{id}.json"))).expect("after");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.folder.updated_at, first.folder.updated_at);
        assert_eq!(before_duplicate, after_duplicate);

        let second = repository.add_item(&id, metadata(2)).expect("second add");
        assert_eq!(second.folder.items.len(), 2);
        assert_eq!(second.folder.items[0].bvid, metadata(2).bvid);
        assert_eq!(second.folder.items[1].bvid, metadata(1).bvid);
    }

    #[test]
    fn favorites_repository_serializes_concurrent_adds_without_loss() {
        let directory = TestDirectory::new();
        let repository = Arc::new(directory.repository());
        let created = repository.create("Concurrent", "").expect("create");
        let id = folder_id(&created).expect("id").to_owned();
        let barrier = Arc::new(Barrier::new(9));
        let mut threads = Vec::new();
        for index in 0..8 {
            let repository = Arc::clone(&repository);
            let id = id.clone();
            let barrier = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                barrier.wait();
                repository.add_item(&id, metadata(index)).expect("add")
            }));
        }
        barrier.wait();
        for worker in threads {
            assert!(!worker.join().expect("join").duplicate);
        }

        let stored = repository.get(&id).expect("stored");
        assert_eq!(stored.items.len(), 8);
        for index in 0..8 {
            assert!(stored
                .items
                .iter()
                .any(|item| item.bvid == metadata(index).bvid));
        }
    }

    #[test]
    fn favorites_repository_lists_newest_first_and_reports_corrupt_files() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let older_id = "11111111-1111-4111-8111-111111111111";
        let newer_id = "22222222-2222-4222-8222-222222222222";
        let mut older = fixture_folder(older_id);
        older.updated_at = "2026-01-02T03:04:05Z".to_owned();
        repository.save(older).expect("older");
        let mut newer = fixture_folder(newer_id);
        newer.updated_at = "2026-01-02T05:04:05+0100".to_owned();
        repository.save(newer).expect("newer");

        let corrupt_path = directory
            .0
            .join("33333333-3333-4333-8333-333333333333.json");
        let corrupt = b"{ intentionally broken";
        fs::write(&corrupt_path, corrupt).expect("corrupt fixture");
        let result = repository.list().expect("list");
        assert_eq!(result.folders.len(), 2);
        assert_eq!(folder_id(&result.folders[0]).expect("newer id"), newer_id);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].starts_with("33333333-3333-4333-8333-333333333333.json："));
        assert_eq!(fs::read(corrupt_path).expect("corrupt intact"), corrupt);
    }

    #[test]
    fn favorites_repository_validates_ids_and_folders_before_writing() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        assert!(is_legacy_folder_id("ABCDEF12----------------------------"));
        assert!(!is_legacy_folder_id("123456789012345678901234567890123456"));
        assert!(matches!(
            repository.get("../outside"),
            Err(RepositoryError::InvalidId)
        ));

        let id = "44444444-4444-4444-8444-444444444444";
        let mut invalid = fixture_folder(id);
        invalid.name = "   ".to_owned();
        assert!(matches!(
            repository.save(invalid),
            Err(RepositoryError::Validation(ValidationError::InvalidName))
        ));
        assert!(!directory.0.exists());
    }

    #[test]
    fn favorites_repository_cleans_temporary_file_after_replace_failure() {
        let directory = TestDirectory::new();
        fs::create_dir_all(&directory.0).expect("directory");
        let id = "55555555-5555-4555-8555-555555555555";
        fs::create_dir(directory.0.join(format!("{id}.json"))).expect("blocking target");
        assert!(matches!(
            directory.repository().save(fixture_folder(id)),
            Err(RepositoryError::Io(_))
        ));
        let temporary_files = fs::read_dir(&directory.0)
            .expect("entries")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[test]
    fn favorites_repository_json_is_schema_v1_and_pretty_printed() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let folder = repository.create("Folder", "").expect("create");
        let id = folder_id(&folder).expect("id");
        let contents = fs::read_to_string(directory.0.join(format!("{id}.json"))).expect("json");
        assert!(contents.contains("\n  \"schemaVersion\": 1"));
        assert_eq!(
            serde_json::from_str::<Value>(&contents).expect("parse")["schemaVersion"],
            json!(1)
        );
    }

    #[test]
    fn favorites_repository_timestamps_match_javascript_iso_shape() {
        fn assert_iso_milliseconds(value: &str) {
            let bytes = value.as_bytes();
            assert_eq!(bytes.len(), 24);
            assert_eq!(bytes[19], b'.');
            assert!(bytes[20..23].iter().all(u8::is_ascii_digit));
            assert_eq!(bytes[23], b'Z');
        }

        let directory = TestDirectory::new();
        let repository = directory.repository();
        let created = repository.create("Folder", "").expect("create");
        assert_iso_milliseconds(&created.created_at);
        assert_iso_milliseconds(&created.updated_at);
        let id = folder_id(&created).expect("id").to_owned();
        let added = repository.add_item(&id, metadata(1)).expect("add").folder;
        assert_iso_milliseconds(&added.updated_at);
        assert_iso_milliseconds(&added.items[0].added_at);
    }
}
