use crate::favorites::{validate_import, FavoriteFolder};
use crate::bilibili::{extract_bvid, select_video_metadata};
use crate::bilibili_http::BilibiliHttpClient;
use crate::repositories::favorites::{FavoriteMetadata, FavoritesRepository, RepositoryError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteCreateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFavoriteItem {
    pub bvid: String,
    pub title: String,
    pub owner: String,
    pub duration: Option<f64>,
    pub cover_url: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteListResponse {
    pub folders: Vec<FavoriteFolder>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteAddResponse {
    pub folder: FavoriteFolder,
    pub duplicate: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteImportPreviewResponse {
    pub folders: Vec<FavoriteFolder>,
    pub preview: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteImportCommitResponse {
    pub folders: Vec<FavoriteFolder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteExportBundle {
    pub schema_version: u8,
    #[serde(rename = "type")]
    pub bundle_type: &'static str,
    pub folders: Vec<FavoriteFolder>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FavoriteCommandError {
    pub message: String,
}

impl From<RepositoryError> for FavoriteCommandError {
    fn from(error: RepositoryError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

#[tauri::command]
pub fn favorites_list(
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteListResponse, FavoriteCommandError> {
    list(repository.inner())
}

#[tauri::command]
pub fn favorites_get(
    folder_id: String,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    get(repository.inner(), &folder_id)
}

#[tauri::command]
pub fn favorites_create(
    request: FavoriteCreateRequest,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    create(repository.inner(), request)
}

#[tauri::command]
pub fn favorites_add_resolved_item(
    folder_id: String,
    item: ResolvedFavoriteItem,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteAddResponse, FavoriteCommandError> {
    add_resolved_item(repository.inner(), &folder_id, item)
}

#[tauri::command]
pub async fn favorites_add_item(
    folder_id: String,
    input: String,
    repository: State<'_, FavoritesRepository>,
    client: State<'_, BilibiliHttpClient>,
) -> Result<FavoriteAddResponse, FavoriteCommandError> {
    add_item(repository.inner(), client.inner(), &folder_id, &input).await
}

async fn add_item(
    repository: &FavoritesRepository,
    client: &BilibiliHttpClient,
    folder_id: &str,
    input: &str,
) -> Result<FavoriteAddResponse, FavoriteCommandError> {
    // Legacy validates the destination before requesting metadata, including duplicates.
    repository.get(folder_id)?;
    let bvid = extract_bvid(input).ok_or_else(|| FavoriteCommandError {
        message: "请输入有效的 BV 号或 Bilibili 视频链接".to_owned(),
    })?;
    let view = client.fetch_view(&bvid).await.map_err(|error| FavoriteCommandError {
        message: super::media::map_view_error(error),
    })?;
    let metadata = select_video_metadata(&view, &bvid);
    if !metadata.has_usable_cid() {
        return Err(FavoriteCommandError { message: "未找到视频分 P 信息".to_owned() });
    }
    add_resolved_item(repository, folder_id, ResolvedFavoriteItem {
        bvid: metadata.bvid,
        title: metadata.title,
        owner: metadata.owner,
        duration: metadata.duration.as_ref().and_then(Value::as_f64),
        cover_url: metadata.cover_url,
    })
}

#[tauri::command]
pub fn favorites_import_preview(
    data: Value,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteImportPreviewResponse, FavoriteCommandError> {
    import_preview(repository.inner(), &data)
}

#[tauri::command]
pub fn favorites_import_commit(
    data: Value,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteImportCommitResponse, FavoriteCommandError> {
    import_commit(repository.inner(), &data)
}

#[tauri::command]
pub fn favorites_export_folder_data(
    folder_id: String,
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    export_folder_data(repository.inner(), &folder_id)
}

#[tauri::command]
pub fn favorites_export_all_data(
    repository: State<'_, FavoritesRepository>,
) -> Result<FavoriteExportBundle, FavoriteCommandError> {
    export_all_data(repository.inner())
}

fn list(repository: &FavoritesRepository) -> Result<FavoriteListResponse, FavoriteCommandError> {
    let result = repository.list()?;
    Ok(FavoriteListResponse {
        folders: result.folders,
        errors: result.errors,
    })
}

fn get(
    repository: &FavoritesRepository,
    folder_id: &str,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    Ok(repository.get(folder_id)?)
}

fn create(
    repository: &FavoritesRepository,
    request: FavoriteCreateRequest,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    Ok(repository.create(&request.name, &request.description)?)
}

fn add_resolved_item(
    repository: &FavoritesRepository,
    folder_id: &str,
    item: ResolvedFavoriteItem,
) -> Result<FavoriteAddResponse, FavoriteCommandError> {
    let result = repository.add_item(
        folder_id,
        FavoriteMetadata {
            bvid: item.bvid,
            title: item.title,
            owner: item.owner,
            duration: item.duration,
            cover_url: item.cover_url,
        },
    )?;
    Ok(FavoriteAddResponse {
        folder: result.folder,
        duplicate: result.duplicate,
    })
}

fn import_preview(
    _repository: &FavoritesRepository,
    data: &Value,
) -> Result<FavoriteImportPreviewResponse, FavoriteCommandError> {
    let folders = validate_import(data).map_err(|error| FavoriteCommandError {
        message: error.to_string(),
    })?;
    Ok(FavoriteImportPreviewResponse {
        folders,
        preview: true,
    })
}

fn import_commit(
    repository: &FavoritesRepository,
    data: &Value,
) -> Result<FavoriteImportCommitResponse, FavoriteCommandError> {
    #[cfg(all(debug_assertions, target_os = "windows"))]
    if std::env::var("P8_T5_PARITY").as_deref() == Ok("1")
        && data["folders"][0]["name"] == "P8-T5 partial first"
        && data["folders"][1]["name"] == "P8-T5 partial second"
    {
        let mut writes = 0;
        return import_commit_with_id_generator(repository, data, || {
            writes += 1;
            if writes == 2 { "p8-t5-invalid-second-id".to_owned() }
            else { Uuid::new_v4().to_string() }
        });
    }
    import_commit_with_id_generator(repository, data, || Uuid::new_v4().to_string())
}

fn import_commit_with_id_generator(
    repository: &FavoritesRepository,
    data: &Value,
    mut generate_id: impl FnMut() -> String,
) -> Result<FavoriteImportCommitResponse, FavoriteCommandError> {
    let folders = validate_import(data).map_err(|error| FavoriteCommandError {
        message: error.to_string(),
    })?;
    let mut saved = Vec::with_capacity(folders.len());
    for mut folder in folders {
        folder.id = Some(Value::String(generate_id()));
        match repository.save(folder) {
            Ok(folder) => saved.push(folder),
            Err(error) => {
                return Ok(FavoriteImportCommitResponse {
                    error: Some(format!(
                        "已导入 {} 个收藏夹，后续写入失败：{error}",
                        saved.len()
                    )),
                    folders: saved,
                });
            }
        }
    }
    Ok(FavoriteImportCommitResponse {
        folders: saved,
        error: None,
    })
}

fn export_folder_data(
    repository: &FavoritesRepository,
    folder_id: &str,
) -> Result<FavoriteFolder, FavoriteCommandError> {
    get(repository, folder_id)
}

fn export_all_data(
    repository: &FavoritesRepository,
) -> Result<FavoriteExportBundle, FavoriteCommandError> {
    let result = repository.list()?;
    if !result.errors.is_empty() {
        return Err(FavoriteCommandError {
            message: result.errors.join("\n"),
        });
    }
    Ok(FavoriteExportBundle {
        schema_version: 1,
        bundle_type: "bili-sound-favorites",
        folders: result.folders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    const VALID_SINGLE: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/valid-single-folder.json");
    const MULTI_FOLDER: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/multi-folder.json");
    const PARTIAL_IMPORT: &str =
        include_str!("../../../referenceCODE/test/fixtures/favorites/partial-import.json");

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "everything-player-favorites-command-test-{}",
                Uuid::new_v4()
            )))
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

    fn fixture(name: &str) -> Value {
        serde_json::from_str(match name {
            "single" => VALID_SINGLE,
            "multi" => MULTI_FOLDER,
            "partial" => PARTIAL_IMPORT,
            _ => unreachable!("known fixture"),
        })
        .expect("fixture JSON")
    }

    fn folder_id(folder: &FavoriteFolder) -> &str {
        folder
            .id
            .as_ref()
            .and_then(Value::as_str)
            .expect("folder id")
    }

    fn resolved_item() -> ResolvedFavoriteItem {
        ResolvedFavoriteItem {
            bvid: "BV1A2B3C4D5E".to_owned(),
            title: "Resolved title".to_owned(),
            owner: "Resolved owner".to_owned(),
            duration: Some(123.5),
            cover_url: "https://example.com/cover.jpg".to_owned(),
        }
    }

    #[tokio::test]
    async fn favorites_metadata_add_never_requests_playurl_and_does_not_write_on_view_failure() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
        use std::time::Duration;

        for play_response in [r#"{"code":-1,"message":"playurl unavailable"}"#,
                              r#"{"code":0,"data":{"dash":{}}}"#] {
            let directory = TestDirectory::new();
            let repository = directory.repository();
            let folder = create(&repository, FavoriteCreateRequest {
                name: "Metadata only".to_owned(), description: String::new(),
            }).unwrap();
            let id = folder_id(&folder).to_owned();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let origin = format!("http://{}/", listener.local_addr().unwrap()).parse().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&requests);
            let stopped = Arc::new(AtomicBool::new(false));
            let stop = Arc::clone(&stopped);
            let server = std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5)); continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 1024];
                    while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
                        let n = stream.read(&mut buffer).unwrap();
                        if n == 0 { break; }
                        bytes.extend_from_slice(&buffer[..n]);
                    }
                    let request = String::from_utf8(bytes).unwrap();
                    let body = if request.contains("/x/player/playurl") { play_response }
                        else if request.contains("BV1zzzzzzzzz") { r#"{"code":-400,"message":"metadata unavailable"}"# }
                        else { r#"{"code":0,"data":{"bvid":"BV1Q541167Qg","title":"Metadata only","owner":{"name":"UP"},"pages":[{"cid":1,"duration":123}],"pic":"//example.com/cover.jpg"}}"# };
                    captured.lock().unwrap().push(request);
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let client = BilibiliHttpClient::for_local_test(origin, Duration::from_secs(2), 4096).unwrap();
            let added = add_item(&repository, &client, &id, "https://www.bilibili.com/video/BV1Q541167Qg").await.unwrap();
            assert!(!added.duplicate);
            assert_eq!(added.folder.items[0].duration, Some(123.0));
            assert_eq!(added.folder.items[0].cover_url, "https://example.com/cover.jpg");
            assert!(add_item(&repository, &client, &id, "BV1Q541167Qg").await.unwrap().duplicate);
            let before = repository.get(&id).unwrap();
            assert_eq!(add_item(&repository, &client, &id, "BV1zzzzzzzzz").await.unwrap_err().message, "metadata unavailable");
            assert_eq!(repository.get(&id).unwrap(), before);
            assert!(add_item(&repository, &client, "invalid-folder", "BV1Q541167Qg").await.is_err());
            stopped.store(true, Ordering::SeqCst);
            server.join().unwrap();
            let paths = requests.lock().unwrap();
            assert_eq!(paths.len(), 3, "invalid destination must not request metadata");
            assert!(paths.iter().all(|request| request.starts_with("GET /x/web-interface/view?")), "no playurl call, regardless of its response/DASH availability");
            assert!(!serde_json::to_string(&before).unwrap().contains("audioUrl"));
        }
    }

    #[test]
    fn favorites_commands_list_create_get_and_error_shape() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        assert_eq!(
            list(&repository).expect("empty list"),
            FavoriteListResponse {
                folders: vec![],
                errors: vec![]
            }
        );

        let created = create(
            &repository,
            FavoriteCreateRequest {
                name: "  Command folder  ".to_owned(),
                description: String::new(),
            },
        )
        .expect("create");
        let id = folder_id(&created).to_owned();
        assert_eq!(created.name, "Command folder");
        assert_eq!(get(&repository, &id).expect("get"), created);
        assert_eq!(
            get(&repository, "not-an-id").expect_err("invalid id"),
            FavoriteCommandError {
                message: "收藏夹 ID 无效".to_owned()
            }
        );
    }

    #[test]
    fn favorites_add_resolved_item_preserves_dto_and_duplicate_semantics() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let folder = create(
            &repository,
            FavoriteCreateRequest {
                name: "Folder".to_owned(),
                description: "Description".to_owned(),
            },
        )
        .expect("create");
        let id = folder_id(&folder).to_owned();

        let added = add_resolved_item(&repository, &id, resolved_item()).expect("add");
        assert!(!added.duplicate);
        assert_eq!(added.folder.items.len(), 1);
        assert_eq!(added.folder.items[0].bvid, "BV1A2B3C4D5E");
        assert_eq!(added.folder.items[0].owner, "Resolved owner");
        assert_eq!(added.folder.items[0].duration, Some(123.5));
        assert_eq!(
            added.folder.items[0].cover_url,
            "https://example.com/cover.jpg"
        );

        let duplicate = add_resolved_item(&repository, &id, resolved_item()).expect("duplicate");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.folder, added.folder);
    }

    #[test]
    fn favorites_import_preview_is_normalized_without_writing() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let data = fixture("single");
        let source_id = data["id"].as_str().expect("source id");
        let preview = import_preview(&repository, &data).expect("preview");

        assert!(preview.preview);
        assert_eq!(preview.folders.len(), 1);
        assert_eq!(folder_id(&preview.folders[0]), source_id);
        assert_eq!(
            serde_json::to_value(&preview).expect("serialize preview"),
            json!({ "folders": preview.folders, "preview": true })
        );
        assert_eq!(list(&repository).expect("still empty").folders, vec![]);
        assert!(!directory.0.exists());
    }

    #[test]
    fn favorites_import_preview_rejects_invalid_data_before_writing() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let error = import_preview(&repository, &json!({ "schemaVersion": 2 }))
            .expect_err("invalid import");
        assert_eq!(error.message, "导入格式或版本无效");
        assert!(!directory.0.exists());
    }

    #[test]
    fn favorites_import_commit_assigns_new_ids_and_preserves_order() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let data = fixture("multi");
        let source_ids = data["folders"]
            .as_array()
            .expect("folders")
            .iter()
            .map(|folder| folder["id"].as_str().expect("source id").to_owned())
            .collect::<Vec<_>>();

        let result = import_commit(&repository, &data).expect("commit");
        assert_eq!(result.error, None);
        assert_eq!(result.folders.len(), 2);
        assert!(serde_json::to_value(&result)
            .expect("serialize commit")
            .get("error")
            .is_none());
        assert_eq!(result.folders[0].name, data["folders"][0]["name"]);
        assert_eq!(result.folders[1].name, data["folders"][1]["name"]);
        for folder in &result.folders {
            assert!(!source_ids.iter().any(|id| id == folder_id(folder)));
            assert_eq!(repository.get(folder_id(folder)).expect("stored"), *folder);
        }
    }

    #[test]
    fn favorites_import_commit_returns_saved_folders_after_partial_failure() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let first_id = "11111111-1111-4111-8111-111111111111";
        let second_id = "22222222-2222-4222-8222-222222222222";
        fs::create_dir_all(directory.0.join(format!("{second_id}.json"))).expect("blocking path");
        let mut ids = [first_id.to_owned(), second_id.to_owned()].into_iter();

        let result = import_commit_with_id_generator(&repository, &fixture("partial"), || {
            ids.next().expect("generated id")
        })
        .expect("partial response");
        assert_eq!(result.folders.len(), 1);
        assert_eq!(folder_id(&result.folders[0]), first_id);
        assert_eq!(
            repository.get(first_id).expect("first persisted"),
            result.folders[0]
        );
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("已导入 1 个收藏夹，后续写入失败：")));
    }

    #[test]
    fn favorites_export_folder_and_all_use_schema_v1_contract() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        let created = create(
            &repository,
            FavoriteCreateRequest {
                name: "Export".to_owned(),
                description: String::new(),
            },
        )
        .expect("create");
        let id = folder_id(&created);
        assert_eq!(export_folder_data(&repository, id).expect("one"), created);

        let bundle = export_all_data(&repository).expect("all");
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.bundle_type, "bili-sound-favorites");
        assert_eq!(bundle.folders, vec![created.clone()]);
        assert_eq!(
            serde_json::to_value(bundle).expect("serialize"),
            json!({
                "schemaVersion": 1,
                "type": "bili-sound-favorites",
                "folders": [created]
            })
        );
    }

    #[test]
    fn favorites_export_all_rejects_corrupt_repository_data() {
        let directory = TestDirectory::new();
        let repository = directory.repository();
        fs::create_dir_all(&directory.0).expect("directory");
        let file_name = "33333333-3333-4333-8333-333333333333.json";
        fs::write(directory.0.join(file_name), b"{ broken").expect("corrupt file");

        let listed = list(&repository).expect("list reports corrupt file");
        assert_eq!(listed.folders, vec![]);
        assert_eq!(listed.errors.len(), 1);
        assert!(listed.errors[0].starts_with(&format!("{file_name}：")));
        let error = export_all_data(&repository).expect_err("corrupt list");
        assert!(error.message.starts_with(&format!("{file_name}：")));
    }
}
