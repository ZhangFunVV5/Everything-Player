import type {
  FileTransferService,
  JsonFilePickOptions,
} from '../platform/file-transfer-service.js';
import type { FavoritesRepository } from '../repositories/favorites-repository.js';
import type { ServiceFailure, ServiceResult } from '../types/errors.js';
import type {
  FavoriteAddRequest,
  FavoriteAddResponse,
  FavoriteCreateRequest,
  FavoriteFolder,
  FavoriteImportCommitResponse,
  FavoriteImportPreviewResponse,
  FavoriteListResponse,
} from '../types/favorites.js';

export interface FavoriteImportSelection {
  preview: FavoriteImportPreviewResponse;
  data: unknown;
}

function failure<T>(error: ServiceFailure): ServiceResult<T> {
  return { ok: false, error };
}

export class FavoritesApplicationService {
  constructor(
    private readonly repository: FavoritesRepository,
    private readonly fileTransfer: FileTransferService,
  ) {}

  list(): Promise<ServiceResult<FavoriteListResponse>> {
    return this.repository.list();
  }

  get(folderId: string): Promise<ServiceResult<FavoriteFolder>> {
    return this.repository.get(folderId);
  }

  create(request: FavoriteCreateRequest): Promise<ServiceResult<FavoriteFolder>> {
    return this.repository.create(request);
  }

  addItem(
    folderId: string,
    request: FavoriteAddRequest,
  ): Promise<ServiceResult<FavoriteAddResponse>> {
    return this.repository.addItem(folderId, request);
  }

  async selectImport(
    options: JsonFilePickOptions,
  ): Promise<ServiceResult<FavoriteImportSelection | null>> {
    const selection = await this.fileTransfer.selectJsonFile(options);
    if (!selection.ok) return failure(selection.error);
    if (selection.value === null) return { ok: true, value: null };

    const preview = await this.repository.previewImport(selection.value.data);
    if (!preview.ok) return failure(preview.error);
    return {
      ok: true,
      value: { preview: preview.value, data: selection.value.data },
    };
  }

  commitImport(data: unknown): Promise<ServiceResult<FavoriteImportCommitResponse>> {
    return this.repository.commitImport(data);
  }

  async exportFolder(folderId: string, suggestedName: string): Promise<ServiceResult<void>> {
    const result = await this.repository.exportFolder(folderId);
    if (!result.ok) return failure(result.error);
    return this.fileTransfer.saveJsonFile({ suggestedName, data: result.value });
  }

  async exportAll(suggestedName: string): Promise<ServiceResult<void>> {
    const result = await this.repository.exportAll();
    if (!result.ok) return failure(result.error);
    return this.fileTransfer.saveJsonFile({ suggestedName, data: result.value });
  }
}
