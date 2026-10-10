import { invoke } from '@tauri-apps/api/core';
import type { FavoritesRepository } from './favorites-repository.js';
import type { ServiceResult } from '../types/errors.js';
import type {
  FavoriteAddRequest,
  FavoriteAddResponse,
  FavoriteCreateRequest,
  FavoriteExportBundle,
  FavoriteFolder,
  FavoriteImportCommitResponse,
  FavoriteImportPreviewResponse,
  FavoriteListResponse,
} from '../types/favorites.js';

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const message = error.message;
    if (typeof message === 'string') return message;
  }
  return '收藏夹操作失败';
}

function invokeResult<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<ServiceResult<T>> {
  return invoke<T>(command, args)
    .then((value) => ({ ok: true, value }) as ServiceResult<T>)
    .catch((error: unknown) => ({
      ok: false,
      error: { message: errorMessage(error) },
    }));
}

export class TauriFavoritesRepository implements FavoritesRepository {
  list(): Promise<ServiceResult<FavoriteListResponse>> {
    return invokeResult<FavoriteListResponse>('favorites_list');
  }

  get(folderId: string): Promise<ServiceResult<FavoriteFolder>> {
    return invokeResult<FavoriteFolder>('favorites_get', { folderId });
  }

  create(request: FavoriteCreateRequest): Promise<ServiceResult<FavoriteFolder>> {
    return invokeResult<FavoriteFolder>('favorites_create', { request });
  }

  addItem(
    folderId: string,
    request: FavoriteAddRequest,
  ): Promise<ServiceResult<FavoriteAddResponse>> {
    return invokeResult<FavoriteAddResponse>('favorites_add_item', { folderId, input: request.input });
  }

  previewImport(data: unknown): Promise<ServiceResult<FavoriteImportPreviewResponse>> {
    return invokeResult<FavoriteImportPreviewResponse>('favorites_import_preview', { data });
  }

  commitImport(data: unknown): Promise<ServiceResult<FavoriteImportCommitResponse>> {
    return invokeResult<FavoriteImportCommitResponse>('favorites_import_commit', { data });
  }

  exportFolder(folderId: string): Promise<ServiceResult<FavoriteFolder>> {
    return invokeResult<FavoriteFolder>('favorites_export_folder_data', { folderId });
  }

  exportAll(): Promise<ServiceResult<FavoriteExportBundle>> {
    return invokeResult<FavoriteExportBundle>('favorites_export_all_data');
  }
}
