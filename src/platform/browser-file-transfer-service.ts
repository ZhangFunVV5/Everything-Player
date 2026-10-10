import type {
  FileTransferService,
  JsonFilePickOptions,
  JsonFileSelection,
  JsonFileSaveRequest,
} from './file-transfer-service.js';
import type { ServiceResult } from '../types/errors.js';

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export class BrowserFileTransferService implements FileTransferService {
  selectJsonFile(
    options: JsonFilePickOptions,
  ): Promise<ServiceResult<JsonFileSelection | null>> {
    return new Promise((resolve) => {
      const picker = document.createElement('input');
      picker.type = 'file';
      picker.accept = options.accept.join(',');
      picker.addEventListener('cancel', () => resolve({ ok: true, value: null }), { once: true });
      picker.onchange = (): void => {
        void (async (): Promise<void> => {
          try {
            const file = picker.files?.[0];
            if (!file) {
              resolve({ ok: true, value: null });
              return;
            }
            if (file.size > options.maxBytes) {
              resolve({ ok: false, error: { message: '文件不能超过 10 MB' } });
              return;
            }
            let data: unknown;
            try {
              data = JSON.parse(await file.text());
            } catch {
              resolve({ ok: false, error: { message: '文件不是有效 JSON' } });
              return;
            }
            resolve({ ok: true, value: { name: file.name, size: file.size, data } });
          } catch (error: unknown) {
            resolve({
              ok: false,
              error: { message: errorMessage(error, '收藏夹操作失败') },
            });
          }
        })();
      };
      try {
        picker.click();
      } catch (error: unknown) {
        resolve({
          ok: false,
          error: { message: errorMessage(error, '收藏夹操作失败') },
        });
      }
    });
  }

  saveJsonFile(request: JsonFileSaveRequest): Promise<ServiceResult<void>> {
    try {
      const content = JSON.stringify(request.data, null, 2) ?? '';
      const blob = new Blob([content], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = request.suggestedName;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      return Promise.resolve({ ok: true, value: undefined });
    } catch (error: unknown) {
      return Promise.resolve({
        ok: false,
        error: { message: errorMessage(error, '收藏夹操作失败') },
      });
    }
  }
}
