import type { FavoritesRepository } from './favorites-repository.js';
import type { ServiceErrorResponse, ServiceResult } from '../types/errors.js';
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

type RequestOptions = {
  method?: 'POST';
  body?: unknown;
};

function isServiceErrorResponse(value: unknown): value is ServiceErrorResponse {
  return typeof value === 'object'
    && value !== null
    && 'error' in value
    && typeof value.error === 'string';
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export class LegacyHttpFavoritesRepository implements FavoritesRepository {
  private async request<T>(
    path: string,
    options: RequestOptions = {},
  ): Promise<ServiceResult<T>> {
    try {
      const request: RequestInit = options.body === undefined
        ? (options.method === undefined ? {} : { method: options.method })
        : {
            method: options.method ?? 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(options.body),
          };
      const response = await fetch(`/api/favorites${path}`, request);
      const result: unknown = await response.json();
      if (!response.ok) {
        return {
          ok: false,
          error: {
            message: isServiceErrorResponse(result) ? result.error : '收藏夹操作失败',
          },
        };
      }
      return { ok: true, value: result as T };
    } catch (error: unknown) {
      return {
        ok: false,
        error: { message: errorMessage(error, '收藏夹操作失败') },
      };
    }
  }

  list(): Promise<ServiceResult<FavoriteListResponse>> {
    return this.request('');
  }

  get(folderId: string): Promise<ServiceResult<FavoriteFolder>> {
    return this.request(`/${folderId}`);
  }

  create(request: FavoriteCreateRequest): Promise<ServiceResult<FavoriteFolder>> {
    return this.request('', { method: 'POST', body: request });
  }

  addItem(
    folderId: string,
    request: FavoriteAddRequest,
  ): Promise<ServiceResult<FavoriteAddResponse>> {
    return this.request(`/${folderId}/items`, { method: 'POST', body: request });
  }

  previewImport(data: unknown): Promise<ServiceResult<FavoriteImportPreviewResponse>> {
    return this.request('/import', {
      method: 'POST',
      body: { data, preview: true },
    });
  }

  commitImport(data: unknown): Promise<ServiceResult<FavoriteImportCommitResponse>> {
    return this.request('/import', {
      method: 'POST',
      body: { data },
    });
  }

  exportFolder(folderId: string): Promise<ServiceResult<FavoriteFolder>> {
    return this.request(`/${folderId}/export`);
  }

  exportAll(): Promise<ServiceResult<FavoriteExportBundle>> {
    return this.request('/export');
  }
}
