import type { ServiceResult } from '../types/errors.js';
import type { FavoriteExportBundle, FavoriteFolder } from '../types/favorites.js';

export interface JsonFileSelection {
  name: string;
  size: number;
  data: unknown;
}

export interface JsonFilePickOptions {
  accept: readonly string[];
  maxBytes: number;
}

export interface JsonFileSaveRequest {
  suggestedName: string;
  data: FavoriteFolder | FavoriteExportBundle;
}

export interface FileTransferService {
  selectJsonFile(
    options: JsonFilePickOptions,
  ): Promise<ServiceResult<JsonFileSelection | null>>;
  saveJsonFile(request: JsonFileSaveRequest): Promise<ServiceResult<void>>;
}
