export const OVERSIZED_MIN_BYTES = 10 * 1024 * 1024 + 1024;

export function createOversizedImportBody() {
  const padding = 'x'.repeat(OVERSIZED_MIN_BYTES + 4096);
  return JSON.stringify({
    schemaVersion: 1,
    type: 'bili-sound-favorites',
    folders: [],
    padding
  });
}
