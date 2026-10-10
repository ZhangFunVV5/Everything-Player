import { isTauri } from '@tauri-apps/api/core';
import { TauriFileTransferService } from './platform/tauri-file-transfer-service.js';
import { TauriLegacyMigrationService } from './platform/tauri-legacy-migration-service.js';
import { TauriMediaService } from './platform/tauri-media-service.js';
import { TauriFavoritesRepository } from './repositories/tauri-favorites-repository.js';
import { FavoritesApplicationService } from './services/favorites-application-service.js';
import { LegacyMigrationApplicationService } from './services/legacy-migration-application-service.js';
import { PlayerApplicationService } from './services/player-application-service.js';

export interface ApplicationServices {
  player: PlayerApplicationService;
  favorites: FavoritesApplicationService;
  legacyMigration: LegacyMigrationApplicationService | null;
}

export function createApplicationServices(): ApplicationServices {
  const packaged = typeof __TAURI_PACKAGED__ !== 'undefined' && __TAURI_PACKAGED__;
  const tauriRuntime = isTauri();
  if (packaged && !tauriRuntime) {
    throw new Error('Tauri 打包应用未检测到 Tauri 运行时，无法初始化原生服务。');
  }
  if (!tauriRuntime) {
    throw new Error('未检测到 Tauri 运行时，请通过 Tauri 启动应用。');
  }
  const media = new TauriMediaService();
  return {
    player: new PlayerApplicationService(media),
    favorites: new FavoritesApplicationService(
      new TauriFavoritesRepository(),
      new TauriFileTransferService(),
    ),
    legacyMigration: new LegacyMigrationApplicationService(new TauriLegacyMigrationService()),
  };
}
