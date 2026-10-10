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

export interface FavoritesRepository {
  list(): Promise<ServiceResult<FavoriteListResponse>>;
  get(folderId: string): Promise<ServiceResult<FavoriteFolder>>;
  create(request: FavoriteCreateRequest): Promise<ServiceResult<FavoriteFolder>>;
  addItem(
    folderId: string,
    request: FavoriteAddRequest,
  ): Promise<ServiceResult<FavoriteAddResponse>>;
  previewImport(data: unknown): Promise<ServiceResult<FavoriteImportPreviewResponse>>;
  commitImport(data: unknown): Promise<ServiceResult<FavoriteImportCommitResponse>>;
  exportFolder(folderId: string): Promise<ServiceResult<FavoriteFolder>>;
  exportAll(): Promise<ServiceResult<FavoriteExportBundle>>;
}
