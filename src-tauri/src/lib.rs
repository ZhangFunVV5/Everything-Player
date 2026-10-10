pub mod commands;
pub mod bilibili;
pub mod bilibili_http;
pub mod favorites;
pub mod repositories;

#[cfg(all(debug_assertions, target_os = "windows"))]
mod media_transport_proof;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let builder = tauri::Builder::default();

  #[cfg(all(debug_assertions, target_os = "windows"))]
  let builder = builder.on_page_load(|window, _| {
    if window.label() == "main"
      && std::env::var("P8_T5_AUTOPLAY_DENIED").as_deref() == Ok("1") {
      let _ = window.eval(r#"
        if (!window.__p8T5AutoplayInstalled) {
          window.__p8T5AutoplayInstalled = true;
          const original = HTMLMediaElement.prototype.play;
          let rejected = false;
          HTMLMediaElement.prototype.play = function() {
            if (!rejected) {
              rejected = true;
              return Promise.reject(new DOMException('P8-T5 autoplay denied', 'NotAllowedError'));
            }
            return original.call(this);
          };
        }
      "#);
    }
  });

  #[cfg(all(debug_assertions, target_os = "windows"))]
  let builder = builder.register_asynchronous_uri_scheme_protocol(
    "media-proof",
    |_ctx, request, responder| {
      media_transport_proof::respond(request, responder);
    },
  );

  builder
    .setup(|app| {
      let repository = repositories::favorites::FavoritesRepository::from_app(app.handle())?;
      app.manage(repository);
      app.manage(commands::migration::LegacyMigrationState::default());
      app.manage(bilibili_http::BilibiliHttpClient::new()?);
      let media_state = commands::media::MediaResolveState::default();
      media_state.start_cleanup_task();
      app.manage(media_state);
      app.manage(commands::media_transport::MediaUpstreamClient::new()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::Other, message))?);
      app.manage(commands::media_transport::MediaTransferState::default());

      #[cfg(all(debug_assertions, target_os = "windows"))]
      tauri::WebviewWindowBuilder::new(
        app,
        "media-transport-proof",
        tauri::WebviewUrl::CustomProtocol(
          "media-proof://localhost/proof.html"
            .parse()
            .expect("synthetic proof page URL"),
        ),
      )
      .title("P7-T1 synthetic media transport proof")
      .inner_size(900.0, 760.0)
      .build()?;

      Ok(())
    })
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
      commands::favorites::favorites_list,
      commands::favorites::favorites_get,
      commands::favorites::favorites_create,
      commands::favorites::favorites_add_resolved_item,
      commands::favorites::favorites_add_item,
      commands::favorites::favorites_import_preview,
      commands::favorites::favorites_import_commit,
      commands::favorites::favorites_export_folder_data,
      commands::favorites::favorites_export_all_data,
      commands::file_transfer::favorites_select_json_file,
      commands::file_transfer::favorites_save_json_file,
      commands::migration::favorites_migration_select_legacy_directory,
      commands::migration::favorites_migration_cancel,
      commands::migration::favorites_migration_commit,
      commands::media::media_resolve,
      commands::media_transport::media_audio_open,
      commands::media_transport::media_audio_read,
      commands::media_transport::media_audio_cancel
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
