fn main() {
    let app_manifest = tauri_build::AppManifest::new().commands(&[
        "favorites_list",
        "favorites_get",
        "favorites_create",
        "favorites_add_resolved_item",
        "favorites_add_item",
        "favorites_import_preview",
        "favorites_import_commit",
        "favorites_export_folder_data",
        "favorites_export_all_data",
        "favorites_select_json_file",
        "favorites_save_json_file",
        "favorites_migration_select_legacy_directory",
        "favorites_migration_cancel",
        "favorites_migration_commit",
        "media_resolve",
        "media_audio_open",
        "media_audio_read",
        "media_audio_cancel",
    ]);
    let attributes = tauri_build::Attributes::new().app_manifest(app_manifest);
    tauri_build::try_build(attributes).expect("failed to generate Tauri application ACL");
}
