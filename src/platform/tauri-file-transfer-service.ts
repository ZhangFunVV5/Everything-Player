import { invoke } from '@tauri-apps/api/core';
import type {
  FileTransferService,
  JsonFilePickOptions,
  JsonFileSaveRequest,
  JsonFileSelection,
} from './file-transfer-service.js';
import type { ServiceResult } from '../types/errors.js';

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const message = error.message;
    if (typeof message === 'string') return message;
  }
  return '收藏夹操作失败';
}

export class TauriFileTransferService implements FileTransferService {
  selectJsonFile(
    _options: JsonFilePickOptions,
  ): Promise<ServiceResult<JsonFileSelection | null>> {
    return invoke<JsonFileSelection | null>('favorites_select_json_file')
      .then((value) => ({ ok: true, value }) as ServiceResult<JsonFileSelection | null>)
      .catch((error: unknown) => ({
        ok: false,
        error: { message: errorMessage(error) },
      }));
  }

  saveJsonFile(request: JsonFileSaveRequest): Promise<ServiceResult<void>> {
    return invoke<void>('favorites_save_json_file', { request })
      .then(() => ({ ok: true, value: undefined }) as ServiceResult<void>)
      .catch((error: unknown) => ({
        ok: false,
        error: { message: errorMessage(error) },
      }));
  }
}
