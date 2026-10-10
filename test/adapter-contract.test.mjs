import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { isTauri } from '@tauri-apps/api/core';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { resolveBvid } from '../referenceCODE/server.mjs';

import { BrowserFileTransferService } from '../build/adapter-contract-test/platform/browser-file-transfer-service.js';
import { LegacyHttpMediaService } from '../build/adapter-contract-test/platform/legacy-http-media-service.js';
import { TauriFileTransferService } from '../build/adapter-contract-test/platform/tauri-file-transfer-service.js';
import { TauriLegacyMigrationService } from '../build/adapter-contract-test/platform/tauri-legacy-migration-service.js';
import { TauriMediaService } from '../build/adapter-contract-test/platform/tauri-media-service.js';
import { createApplicationServices } from '../build/adapter-contract-test/composition-root.js';
import { LegacyHttpFavoritesRepository } from '../build/adapter-contract-test/repositories/legacy-http-favorites-repository.js';
import { TauriFavoritesRepository } from '../build/adapter-contract-test/repositories/tauri-favorites-repository.js';
import { migrationNotification } from '../build/adapter-contract-test/favorites.js';
import { PlayerApplicationService } from '../build/adapter-contract-test/services/player-application-service.js';
import { TauriAudioPlayback } from '../build/adapter-contract-test/platform/tauri-audio-playback.js';
import { runVersionedResolve } from '../build/adapter-contract-test/services/versioned-resolve.js';

test('failed newer resolve restores the displayed track after an older async audio load finishes', async () => {
  const current = { audioUrl: 'current' };
  const older = { audioUrl: 'older' };
  let releaseOlder;
  let olderEntered;
  const olderGate = new Promise((resolve) => { releaseOlder = resolve; });
  const olderStarted = new Promise((resolve) => { olderEntered = resolve; });
  const disposed = [];
  const initialTimes = [];
  const audio = {
    src: '', currentTime: 0, paused: true, plays: 0,
    pause() { this.paused = true; },
    async play() { this.paused = false; this.plays += 1; },
  };
  const media = {
    resolve: async (input) => input === 'A'
      ? { ok: true, value: older }
      : { ok: false, error: { message: 'B failed' } },
    createPlayback: async (_audio, video, _onError, initialTime) => {
      initialTimes.push([video.audioUrl, initialTime]);
      audio.src = video.audioUrl;
      if (video === older) {
        olderEntered();
        await olderGate;
      }
      return {
        seek: async () => {}, resume: async () => {},
        dispose: async () => {
          disposed.push(video.audioUrl);
          if (audio.src === video.audioUrl) audio.src = '';
        },
      };
    },
  };
  const player = new PlayerApplicationService(media);
  await player.loadAudio(audio, current, () => {});
  player.commitPlayback();
  audio.currentTime = 73;
  audio.paused = false;

  let requestVersion = 0;
  let displayed = current;
  let failure = null;
  let loading = false;
  const load = (input) => {
    const version = ++requestVersion;
    return runVersionedResolve({
      resolve: () => player.resolveForPlayback(input),
      isCurrent: () => version === requestVersion,
      snapshot: () => displayed,
      start: () => { player.invalidatePendingAudioLoad(); loading = true; },
      succeed: async (video) => {
        await player.loadAudio(audio, video, () => {});
        if (version !== requestVersion) throw new Error('superseded');
        displayed = video;
        player.commitPlayback();
        loading = false;
      },
      restore: (snapshot) => { displayed = snapshot; },
      fail: async (message) => {
        await player.restoreAfterFailedResolve(audio, () => {}, () => version === requestVersion);
        if (version !== requestVersion) return;
        failure = message;
        loading = false;
      },
    });
  };
  const first = load('A');
  await olderStarted;
  const second = load('B');
  await Promise.resolve();
  assert.equal(displayed, current);
  assert.equal(audio.src, 'older');
  releaseOlder();
  assert.equal(await first, false);
  assert.equal(await second, false);
  assert.equal(failure, 'B failed');
  assert.equal(loading, false);
  assert.equal(displayed, current);
  assert.equal(audio.src, 'current');
  assert.equal(audio.currentTime, 73);
  assert.equal(audio.paused, false);
  assert.ok(audio.plays > 0);
  assert.deepEqual(disposed, ['current', 'older']);
  assert.deepEqual(initialTimes.at(-1), ['current', 73]);
});

test('far-position recovery opens the indexed fragment Range after bounded index reads', async () => {
  const bytes = new Uint8Array(80);
  const view = new DataView(bytes.buffer);
  const box = (offset, size, type) => {
    view.setUint32(offset, size);
    for (let i = 0; i < 4; i += 1) bytes[offset + 4 + i] = type.charCodeAt(i);
  };
  box(0, 8, 'ftyp'); box(8, 8, 'moov'); box(16, 56, 'sidx'); box(72, 8, 'moof');
  view.setUint32(32, 1000);
  view.setUint16(46, 2);
  view.setUint32(48, 100); view.setUint32(52, 5000);
  view.setUint32(60, 120); view.setUint32(64, 5000);
  const opens = [];
  let firstRead = true;
  const previousMediaSource = globalThis.MediaSource;
  const previousWindow = globalThis.window;
  const previousCreateObjectURL = URL.createObjectURL;
  const previousRevokeObjectURL = URL.revokeObjectURL;
  class FakeBuffer extends EventTarget {
    updating = false;
    buffered = { length: 0 };
    appendBuffer() {}
  }
  class FakeMediaSource extends EventTarget {
    static isTypeSupported() { return true; }
    readyState = 'open';
    constructor() { super(); queueMicrotask(() => this.dispatchEvent(new Event('sourceopen'))); }
    addSourceBuffer() { return new FakeBuffer(); }
  }
  const audio = { src: '', currentTime: 0, buffered: { length: 0 }, load() {}, addEventListener() {}, removeEventListener() {}, removeAttribute() {} };
  try {
    globalThis.window = globalThis;
    globalThis.MediaSource = FakeMediaSource;
    URL.createObjectURL = () => 'blob:test';
    URL.revokeObjectURL = () => {};
    mockIPC((command, args) => {
      if (command === 'media_audio_open') {
        opens.push(args.range);
        return { requestId: args.requestId, status: args.range ? 206 : 200, headers: { 'content-type': 'audio/mp4' } };
      }
      if (command === 'media_audio_read') {
        if (firstRead) { firstRead = false; return bytes; }
        return new Uint8Array();
      }
      if (command === 'media_audio_cancel') return true;
      throw new Error(command);
    });
    const playback = new TauriAudioPlayback(audio, 'synthetic-token', () => {});
    await playback.start(7);
    assert.deepEqual(opens, [null, 'bytes=172-']);
    assert.equal(audio.currentTime, 7);
    await playback.dispose();
  } finally {
    clearMocks();
    globalThis.MediaSource = previousMediaSource;
    globalThis.window = previousWindow;
    URL.createObjectURL = previousCreateObjectURL;
    URL.revokeObjectURL = previousRevokeObjectURL;
  }
});

test('player playback sessions discard a superseded async attachment', async () => {
  const calls = [];
  let finishFirst;
  const first = new Promise((resolve) => { finishFirst = resolve; });
  let attachments = 0;
  const media = {
    resolve: async () => ({ ok: false, error: { message: 'unused' } }),
    createPlayback: async (_audio, video) => {
      attachments += 1;
      if (attachments === 1) await first;
      calls.push(`attach:${video.audioUrl}`);
      return {
        seek: async () => {},
        resume: async () => {},
        dispose: async () => { calls.push(`dispose:${video.audioUrl}`); },
      };
    },
  };
  const player = new PlayerApplicationService(media);
  const audio = { currentTime: 0, paused: true, pause() { this.paused = true; } };
  const oldLoad = player.loadAudio(audio, { audioUrl: 'old' }, () => {});
  for (let attempt = 0; attempt < 5 && attachments === 0; attempt += 1) await Promise.resolve();
  assert.equal(attachments, 1);
  const newLoad = player.loadAudio(audio, { audioUrl: 'new' }, () => {});
  finishFirst();
  await Promise.all([oldLoad, newLoad]);
  assert.deepEqual(calls, ['attach:old', 'dispose:old', 'attach:new']);
  await player.disposeAudio();
  assert.deepEqual(calls.at(-1), 'dispose:new');
});

const FIXTURE_DIR = new URL('../referenceCODE/test/fixtures/favorites/', import.meta.url);

async function readFixture(name) {
  return JSON.parse(await readFile(new URL(name, FIXTURE_DIR), 'utf8'));
}

function jsonResponse(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

test('legacy migration record failure is shown without an unconditional success notice', () => {
  const message = migrationNotification({
    files: [{
      name: 'legacy.json',
      size: 321,
      status: 'migrated',
      folderCount: 1,
      targetFolderIds: ['aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'],
    }],
    importedFolders: 1,
    recordError: '迁移记录写入失败',
  });

  assert.match(message, /迁移记录未能保存/);
  assert.match(message, /已复制的收藏夹已保留/);
  assert.match(message, /不会自动删除/);
  assert.doesNotMatch(message, /已迁移 1 个收藏夹/);
});

function throwingJsonResponse(error) {
  return {
    ok: true,
    json: async () => { throw error; },
  };
}

function assertFailure(result, message) {
  assert.deepEqual(result, { ok: false, error: { message } });
}

test('Tauri adapters map narrow invoke commands and typed DTOs', { concurrency: false }, async () => {
  const validFolder = await readFixture('valid-single-folder.json');
  const multiFolder = await readFixture('multi-folder.json');
  const partialImport = await readFixture('partial-import.json');
  const originalFetch = globalThis.fetch;
  const originalDocument = globalThis.document;
  const originalWindow = globalThis.window;
  const originalRuntimeFlag = globalThis.isTauri;
  const calls = [];
  const createRequest = { name: 'Synthetic folder', description: 'Synthetic description' };
  const previewResponse = { folders: multiFolder.folders, preview: true };
  const commitResponse = { folders: [partialImport.folders[0]], error: 'partial import' };
  const migrationPreview = {
    sessionId: 'migration-session',
    files: [{
      name: 'legacy.json',
      size: 321,
      status: 'importable',
      folderCount: 1,
      targetFolderIds: [],
    }],
    canCommit: true,
  };
  const migrationResult = {
    files: [{
      name: 'legacy.json',
      size: 321,
      status: 'migrated',
      folderCount: 1,
      targetFolderIds: ['22222222-2222-4222-8222-222222222222'],
    }],
    importedFolders: 1,
  };
  const resolved = {
    bvid: 'BV1A2B3C4D5E',
    cid: 9101,
    title: 'Synthetic Resolve Video',
    owner: 'Synthetic Resolve UP',
    pageTitle: 'Synthetic First Page',
    duration: 111,
    coverUrl: 'https://example.com/resolve-cover.jpg',
    collection: null,
    audioUrl: 'media-token://11111111-1111-4111-8111-111111111111',
  };
  const responses = {
    media_resolve: resolved,
    favorites_list: { folders: [validFolder], errors: [] },
    favorites_get: validFolder,
    favorites_create: validFolder,
    favorites_add_item: { folder: validFolder, duplicate: false },
    favorites_import_preview: previewResponse,
    favorites_import_commit: commitResponse,
    favorites_export_folder_data: validFolder,
    favorites_export_all_data: multiFolder,
    favorites_select_json_file: {
      name: 'favorites.json',
      size: 321,
      data: validFolder,
    },
    favorites_migration_select_legacy_directory: migrationPreview,
    favorites_migration_commit: migrationResult,
  };
  globalThis.isTauri = true;
  globalThis.fetch = async () => { throw new Error('Tauri adapter must not fetch'); };
  globalThis.document = {
    createElement() { throw new Error('Tauri adapter must not touch the DOM'); },
  };
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: async (command, args) => {
        calls.push({ command, args });
        if (command === 'favorites_save_json_file') return undefined;
        return responses[command];
      },
    },
  };

  try {
    const media = new TauriMediaService();
    assert.deepEqual(await media.resolve('BV1A2B3C4D5E'), { ok: true, value: resolved });

    const favorites = new TauriFavoritesRepository();
    assert.deepEqual(await favorites.list(), { ok: true, value: responses.favorites_list });
    assert.deepEqual(await favorites.get(validFolder.id), { ok: true, value: validFolder });
    assert.deepEqual(await favorites.create(createRequest), { ok: true, value: validFolder });
    assert.deepEqual(await favorites.addItem(validFolder.id, { input: 'BV1A2B3C4D5E' }), {
      ok: true, value: responses.favorites_add_item,
    });
    assert.deepEqual(await favorites.previewImport(multiFolder), { ok: true, value: previewResponse });
    assert.deepEqual(await favorites.commitImport(partialImport), { ok: true, value: commitResponse });
    assert.deepEqual(await favorites.exportFolder(validFolder.id), { ok: true, value: validFolder });
    assert.deepEqual(await favorites.exportAll(), { ok: true, value: multiFolder });

    const fileTransfer = new TauriFileTransferService();
    assert.deepEqual(
      await fileTransfer.selectJsonFile({ accept: ['.json'], maxBytes: 10 * 1024 * 1024 }),
      { ok: true, value: responses.favorites_select_json_file },
    );
    assert.deepEqual(
      await fileTransfer.saveJsonFile({ suggestedName: 'favorites.json', data: validFolder }),
      { ok: true, value: undefined },
    );
    const migration = new TauriLegacyMigrationService();
    assert.deepEqual(await migration.select(), { ok: true, value: migrationPreview });
    assert.deepEqual(await migration.cancel('migration-session'), { ok: true, value: undefined });
    assert.deepEqual(await migration.commit('migration-session'), { ok: true, value: migrationResult });
    assert.deepEqual(calls, [
      { command: 'media_resolve', args: { input: 'BV1A2B3C4D5E' } },
      { command: 'favorites_list', args: {} },
      { command: 'favorites_get', args: { folderId: validFolder.id } },
      { command: 'favorites_create', args: { request: createRequest } },
      { command: 'favorites_add_item', args: { folderId: validFolder.id, input: 'BV1A2B3C4D5E' } },
      { command: 'favorites_import_preview', args: { data: multiFolder } },
      { command: 'favorites_import_commit', args: { data: partialImport } },
      { command: 'favorites_export_folder_data', args: { folderId: validFolder.id } },
      { command: 'favorites_export_all_data', args: {} },
      { command: 'favorites_select_json_file', args: {} },
      {
        command: 'favorites_save_json_file',
        args: { request: { suggestedName: 'favorites.json', data: validFolder } },
      },
      { command: 'favorites_migration_select_legacy_directory', args: {} },
      { command: 'favorites_migration_cancel', args: { sessionId: 'migration-session' } },
      { command: 'favorites_migration_commit', args: { sessionId: 'migration-session' } },
    ]);

    globalThis.window.__TAURI_INTERNALS__.invoke = async () => { throw { message: 'Rust 命令错误' }; };
    assertFailure(await media.resolve('BV1A2B3C4D5E'), 'Rust 命令错误');
    assertFailure(await favorites.list(), 'Rust 命令错误');
    assertFailure(await migration.commit('migration-session'), 'Rust 命令错误');

    for (const [thrown, expected] of [
      ['Rust 字符串错误', 'Rust 字符串错误'],
      [new Error('JavaScript Error 错误'), 'JavaScript Error 错误'],
      [{ message: 'Rust 对象错误' }, 'Rust 对象错误'],
      [{ unexpected: true }, '解析失败'],
      ['https://cdn.example/audio.m4s?token=secret', '解析失败'],
      [{ message: 'media-token://11111111-1111-4111-8111-111111111111' }, '解析失败'],
      [new Error('C:\\private\\credentials.txt'), '解析失败'],
      ['/var/private/credentials.txt', '解析失败'],
    ]) {
      globalThis.window.__TAURI_INTERNALS__.invoke = async () => { throw thrown; };
      assertFailure(await media.resolve('BV1A2B3C4D5E'), expected);
    }
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.document = originalDocument;
    globalThis.window = originalWindow;
    globalThis.isTauri = originalRuntimeFlag;
  }
});

test('native favorite add invokes only the metadata favorite command and preserves failures', async () => {
  const originalWindow = globalThis.window;
  globalThis.window = {};
  const repository = new TauriFavoritesRepository();
  let calls = 0;
  mockIPC((command, args) => {
    assert.equal(command, 'favorites_add_item');
    assert.equal(args.folderId, 'folder');
    calls += 1;
    if (args.input === 'invalid') throw { message: '元数据错误' };
    assert.equal(args.input, 'BV1Q541167Qg');
    if (calls === 3) throw { message: '磁盘写入失败' };
    return { folder: { id: 'folder' }, duplicate: true };
  });
  try {
    assertFailure(await repository.addItem('folder', { input: 'invalid' }), '元数据错误');
    assert.deepEqual(await repository.addItem('folder', { input: 'BV1Q541167Qg' }), {
      ok: true, value: { folder: { id: 'folder' }, duplicate: true },
    });
    assertFailure(await repository.addItem('folder', { input: 'BV1Q541167Qg' }), '磁盘写入失败');
    assert.equal(calls, 3);
  } finally { clearMocks(); globalThis.window = originalWindow; }
});

test('composition root selects Tauri adapters through the official runtime detector', { concurrency: false }, async () => {
  const originalPackaged = globalThis.__TAURI_PACKAGED__;
  globalThis.__TAURI_PACKAGED__ = true;
  const hadRuntimeFlag = Object.hasOwn(globalThis, 'isTauri');
  const originalRuntimeFlag = globalThis.isTauri;
  const originalFetch = globalThis.fetch;
  const originalDocument = globalThis.document;
  const originalWindow = globalThis.window;
  const calls = [];
  let fetchCalls = 0;
  let domCalls = 0;
  const resolved = {
    bvid: 'BV1A2B3C4D5E',
    cid: 9101,
    title: 'Synthetic Resolve Video',
    owner: 'Synthetic Resolve UP',
    pageTitle: 'Synthetic First Page',
    duration: 111,
    coverUrl: 'https://example.com/resolve-cover.jpg',
    collection: null,
    audioUrl: 'media-token://11111111-1111-4111-8111-111111111111',
  };

  globalThis.isTauri = true;
  globalThis.fetch = async () => {
    fetchCalls += 1;
    throw new Error('Tauri composition must not use legacy fetch');
  };
  globalThis.document = {
    createElement() {
      domCalls += 1;
      throw new Error('Tauri composition must not use browser file transfer');
    },
  };
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: async (command, args) => {
        calls.push({ command, args });
        if (command === 'media_resolve') return resolved;
        if (command === 'favorites_list') return { folders: [], errors: [] };
        if (command === 'favorites_select_json_file') return null;
        throw new Error(`Unexpected Tauri command: ${command}`);
      },
    },
  };

  try {
    assert.equal(isTauri(), true);
    const services = createApplicationServices();
    assert.deepEqual(
      await services.player.resolveForPlayback('BV1A2B3C4D5E'),
      { ok: true, value: resolved },
    );
    assert.deepEqual(await services.favorites.list(), { ok: true, value: { folders: [], errors: [] } });
    assert.deepEqual(
      await services.favorites.selectImport({ accept: ['.json'], maxBytes: 10 }),
      { ok: true, value: null },
    );
    assert.equal(fetchCalls, 0);
    assert.equal(domCalls, 0);
    assert.deepEqual(calls, [
      { command: 'media_resolve', args: { input: 'BV1A2B3C4D5E' } },
      { command: 'favorites_list', args: {} },
      { command: 'favorites_select_json_file', args: {} },
    ]);
  } finally {
    if (originalPackaged === undefined) delete globalThis.__TAURI_PACKAGED__;
    else globalThis.__TAURI_PACKAGED__ = originalPackaged;
    globalThis.fetch = originalFetch;
    globalThis.document = originalDocument;
    globalThis.window = originalWindow;
    if (hadRuntimeFlag) globalThis.isTauri = originalRuntimeFlag;
    else delete globalThis.isTauri;
  }
});

test('packaged composition fails explicitly when the Tauri runtime is missing', { concurrency: false }, () => {
  const originalPackaged = globalThis.__TAURI_PACKAGED__;
  const originalRuntimeFlag = globalThis.isTauri;
  const originalFetch = globalThis.fetch;
  let fetchCalls = 0;
  globalThis.__TAURI_PACKAGED__ = true;
  globalThis.isTauri = false;
  globalThis.fetch = () => { fetchCalls += 1; throw new Error('Legacy HTTP used'); };
  try {
    assert.equal(isTauri(), false);
    assert.throws(createApplicationServices, /未检测到 Tauri 运行时/);
    assert.equal(fetchCalls, 0);
  } finally {
    if (originalPackaged === undefined) delete globalThis.__TAURI_PACKAGED__;
    else globalThis.__TAURI_PACKAGED__ = originalPackaged;
    globalThis.isTauri = originalRuntimeFlag;
    globalThis.fetch = originalFetch;
  }
});

test('unpackaged composition requires Tauri instead of selecting Legacy HTTP', { concurrency: false }, () => {
  const originalPackaged = globalThis.__TAURI_PACKAGED__;
  const hadRuntimeFlag = Object.hasOwn(globalThis, 'isTauri');
  const originalRuntimeFlag = globalThis.isTauri;
  const originalFetch = globalThis.fetch;
  let fetchCalls = 0;
  globalThis.__TAURI_PACKAGED__ = false;
  globalThis.isTauri = false;
  globalThis.fetch = () => { fetchCalls += 1; throw new Error('Legacy HTTP used'); };
  try {
    assert.equal(isTauri(), false);
    assert.throws(createApplicationServices, /请通过 Tauri 启动应用/);
    assert.equal(fetchCalls, 0);
  } finally {
    if (originalPackaged === undefined) delete globalThis.__TAURI_PACKAGED__;
    else globalThis.__TAURI_PACKAGED__ = originalPackaged;
    globalThis.fetch = originalFetch;
    if (hadRuntimeFlag) globalThis.isTauri = originalRuntimeFlag;
    else delete globalThis.isTauri;
  }
});
test('legacy media adapter preserves resolve DTO, request shape, and errors', { concurrency: false }, async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  const resolved = {
    bvid: 'BV1A2B3C4D5E',
    cid: 9101,
    title: 'Synthetic Resolve Video',
    owner: 'Synthetic Resolve UP',
    pageTitle: 'Synthetic First Page',
    duration: 111,
    coverUrl: 'https://example.com/resolve-cover.jpg',
    collection: {
      id: 901,
      title: 'Synthetic Collection',
      sections: [{
        id: 1,
        title: 'Synthetic Section',
        episodes: [{
          bvid: 'BV1F2G3H4J5K',
          title: 'Synthetic Episode',
          duration: null,
          coverUrl: 'https://example.com/episode.jpg',
        }],
      }],
    },
    audioUrl: '/api/audio/00000000-0000-4000-8000-000000000000',
  };
  const service = new LegacyHttpMediaService();

  try {
    globalThis.fetch = async (input, options) => {
      calls.push({ input, options });
      return jsonResponse(resolved);
    };
    assert.deepEqual(await service.resolve('BV1A2B3C4D5E'), { ok: true, value: resolved });
    assert.deepEqual(calls[0], {
      input: '/api/resolve',
      options: {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ input: 'BV1A2B3C4D5E' }),
      },
    });

    globalThis.fetch = async () => jsonResponse({ error: '合成后端错误' }, 502);
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '合成后端错误');

    globalThis.fetch = async () => jsonResponse({ message: '没有 error 字段' }, 500);
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '解析失败');

    globalThis.fetch = async () => { throw new Error('合成网络失败'); };
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '合成网络失败');

    globalThis.fetch = async () => { throw '非 Error resolve 异常'; };
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '解析失败');

    globalThis.fetch = async () => throwingJsonResponse(new Error('响应 JSON 损坏'));
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '响应 JSON 损坏');

    globalThis.fetch = async () => throwingJsonResponse('非 Error JSON 异常');
    assertFailure(await service.resolve('BV1A2B3C4D5E'), '解析失败');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('bilibili Legacy and Rust resolvers return semantically equal public DTOs from the shared fixture', { concurrency: false }, async () => {
  const fixture = JSON.parse(await readFile(
    new URL('../src-tauri/test/fixtures/bilibili/resolve-contract.json', import.meta.url),
    'utf8',
  ));
  const originalFetch = globalThis.fetch;
  let legacyDto;

  try {
    globalThis.fetch = async (input) => {
      const url = new URL(input);
      assert.equal(url.origin, 'https://api.bilibili.com');
      if (url.pathname === '/x/web-interface/view') return jsonResponse(fixture.view);
      if (url.pathname === '/x/player/playurl') return jsonResponse(fixture.playurl);
      throw new Error(`Unexpected mocked Legacy endpoint: ${url.pathname}`);
    };
    legacyDto = await resolveBvid(fixture.input);
  } finally {
    globalThis.fetch = originalFetch;
  }

  const legacyToken = legacyDto.audioUrl.match(/^\/api\/audio\/([0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$/i);
  assert.ok(legacyToken, 'Legacy resolver returns its opaque UUID audio address');
  legacyDto = { ...legacyDto, audioUrl: '<opaque-media-token>' };

  const root = fileURLToPath(new URL('../', import.meta.url));
  const rust = spawnSync('cargo', [
    'test',
    '--manifest-path', 'src-tauri/Cargo.toml',
    '--lib',
    'bilibili_mocked_fixture_matches_legacy_dto_headers_selection_and_privacy',
    '--', '--nocapture',
  ], {
    cwd: root,
    encoding: 'utf8',
    env: { ...process.env, BILIBILI_PARITY_JSON: '1' },
    timeout: 120_000,
  });
  assert.equal(rust.error, undefined, rust.error?.message);
  assert.equal(rust.status, 0, `${rust.stdout}\n${rust.stderr}`);

  const output = rust.stdout.match(/^BILIBILI_PARITY_DTO=(.*)$/m);
  assert.ok(output, 'Rust resolver emitted its token-normalized DTO for comparison');
  const rustDto = JSON.parse(output[1]);
  assert.deepEqual(legacyDto, rustDto);
  assert.deepEqual(legacyDto, fixture.expectedResponse);
});

test('legacy and Tauri media adapters share the ServiceFailure error contract', { concurrency: false }, async () => {
  const originalFetch = globalThis.fetch;
  const originalWindow = globalThis.window;
  const legacy = new LegacyHttpMediaService();
  const tauri = new TauriMediaService();

  try {
    globalThis.fetch = async () => jsonResponse({ error: '共同解析错误' }, 400);
    globalThis.window = {
      __TAURI_INTERNALS__: {
        invoke: async () => { throw '共同解析错误'; },
      },
    };
    assert.deepEqual(
      await tauri.resolve('BV1A2B3C4D5E'),
      await legacy.resolve('BV1A2B3C4D5E'),
    );

    globalThis.fetch = async () => jsonResponse({ message: '没有 error 字段' }, 500);
    globalThis.window.__TAURI_INTERNALS__.invoke = async () => { throw { unexpected: true }; };
    assert.deepEqual(
      await tauri.resolve('BV1A2B3C4D5E'),
      await legacy.resolve('BV1A2B3C4D5E'),
    );
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.window = originalWindow;
  }
});

test('legacy favorites adapter preserves all routes, DTOs, fixtures, and errors', { concurrency: false }, async () => {
  const originalFetch = globalThis.fetch;
  const validFolder = await readFixture('valid-single-folder.json');
  const multiFolder = await readFixture('multi-folder.json');
  const partialImport = await readFixture('partial-import.json');
  const calls = [];
  let nextResponse;

  const repository = new LegacyHttpFavoritesRepository();
  globalThis.fetch = async (input, options) => {
    calls.push({ input, options });
    const response = nextResponse;
    nextResponse = undefined;
    if (response && response.throw) throw response.throw;
    return response;
  };

  async function invoke(response, operation) {
    nextResponse = response;
    const result = await operation();
    return { result, call: calls.at(-1) };
  }

  function assertGet(call, path) {
    assert.equal(call.input, path);
    assert.deepEqual(call.options, {});
    assert.equal(call.options.method ?? 'GET', 'GET');
    assert.equal(Object.hasOwn(call.options, 'body'), false);
  }

  function assertPost(call, path, body) {
    assert.equal(call.input, path);
    assert.equal(call.options.method, 'POST');
    assert.deepEqual(call.options.headers, { 'Content-Type': 'application/json' });
    assert.equal(call.options.body, JSON.stringify(body));
  }

  try {
    let outcome = await invoke(jsonResponse({ folders: [validFolder], errors: [] }), () => repository.list());
    assert.deepEqual(outcome.result, { ok: true, value: { folders: [validFolder], errors: [] } });
    assertGet(outcome.call, '/api/favorites');

    outcome = await invoke(jsonResponse(validFolder), () => repository.get(validFolder.id));
    assert.deepEqual(outcome.result, { ok: true, value: validFolder });
    assertGet(outcome.call, `/api/favorites/${validFolder.id}`);

    const createRequest = { name: 'Synthetic Created Folder', description: 'Synthetic description' };
    outcome = await invoke(jsonResponse(validFolder, 201), () => repository.create(createRequest));
    assert.deepEqual(outcome.result, { ok: true, value: validFolder });
    assertPost(outcome.call, '/api/favorites', createRequest);

    const addRequest = { input: validFolder.items[0].bvid };
    outcome = await invoke(jsonResponse({ folder: validFolder, duplicate: false }), () => repository.addItem(validFolder.id, addRequest));
    assert.deepEqual(outcome.result, { ok: true, value: { folder: validFolder, duplicate: false } });
    assertPost(outcome.call, `/api/favorites/${validFolder.id}/items`, addRequest);

    outcome = await invoke(jsonResponse({ folder: validFolder, duplicate: true }), () => repository.addItem(validFolder.id, addRequest));
    assert.deepEqual(outcome.result, { ok: true, value: { folder: validFolder, duplicate: true } });
    assertPost(outcome.call, `/api/favorites/${validFolder.id}/items`, addRequest);

    const previewRequest = { data: multiFolder, preview: true };
    outcome = await invoke(jsonResponse({ folders: multiFolder.folders, preview: true }), () => repository.previewImport(multiFolder));
    assert.deepEqual(outcome.result, { ok: true, value: { folders: multiFolder.folders, preview: true } });
    assertPost(outcome.call, '/api/favorites/import', previewRequest);

    const commitRequest = { data: partialImport };
    const partialResponse = {
      folders: [partialImport.folders[0]],
      error: '已导入 1 个收藏夹，后续写入失败：disk full',
    };
    outcome = await invoke(jsonResponse(partialResponse), () => repository.commitImport(partialImport));
    assert.deepEqual(outcome.result, { ok: true, value: partialResponse });
    assertPost(outcome.call, '/api/favorites/import', commitRequest);

    outcome = await invoke(jsonResponse(validFolder), () => repository.exportFolder(validFolder.id));
    assert.deepEqual(outcome.result, { ok: true, value: validFolder });
    assertGet(outcome.call, `/api/favorites/${validFolder.id}/export`);

    outcome = await invoke(jsonResponse(multiFolder), () => repository.exportAll());
    assert.deepEqual(outcome.result, { ok: true, value: multiFolder });
    assertGet(outcome.call, '/api/favorites/export');

    outcome = await invoke(jsonResponse({ error: '收藏夹后端错误' }, 409), () => repository.list());
    assertFailure(outcome.result, '收藏夹后端错误');

    outcome = await invoke(jsonResponse({ message: '没有有效 error' }, 500), () => repository.list());
    assertFailure(outcome.result, '收藏夹操作失败');

    outcome = await invoke(throwingJsonResponse(new Error('收藏夹响应 JSON 损坏')), () => repository.list());
    assertFailure(outcome.result, '收藏夹响应 JSON 损坏');

    outcome = await invoke(throwingJsonResponse('非 Error JSON 异常'), () => repository.list());
    assertFailure(outcome.result, '收藏夹操作失败');

    outcome = await invoke({ throw: new Error('收藏夹网络失败') }, () => repository.list());
    assertFailure(outcome.result, '收藏夹网络失败');

    outcome = await invoke({ throw: '非 Error 收藏夹异常' }, () => repository.list());
    assertFailure(outcome.result, '收藏夹操作失败');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

function saveBrowserGlobals() {
  return {
    document: globalThis.document,
    Blob: globalThis.Blob,
    URL: globalThis.URL,
    setTimeout: globalThis.setTimeout,
  };
}

function restoreBrowserGlobals(originals) {
  globalThis.document = originals.document;
  globalThis.Blob = originals.Blob;
  globalThis.URL = originals.URL;
  globalThis.setTimeout = originals.setTimeout;
}

function installPickerDocument({ file, action = 'change', clickError, hasClickError = false }) {
  const state = { picker: null };
  globalThis.document = {
    createElement(tag) {
      assert.equal(tag, 'input');
      const picker = {
        type: '',
        accept: '',
        files: file === undefined ? [] : [file],
        onchange: null,
        cancelListener: null,
        addEventListener(name, listener) {
          assert.equal(name, 'cancel');
          this.cancelListener = listener;
        },
        click() {
          if (hasClickError) throw clickError;
          if (action === 'cancel') this.cancelListener();
          else this.onchange();
        },
      };
      state.picker = picker;
      return picker;
    },
  };
  return state;
}

function makeFile({ name, size, text, errorValue }) {
  return {
    name,
    size,
    async text() {
      if (errorValue !== undefined) throw errorValue;
      return text;
    },
  };
}

test('browser file adapter preserves picker, JSON, and download contracts', { concurrency: false }, async () => {
  const validFolder = await readFixture('valid-single-folder.json');
  const originals = saveBrowserGlobals();
  const service = new BrowserFileTransferService();

  try {
    let state = installPickerDocument({
      file: makeFile({
        name: 'valid-single-folder.json',
        size: 321,
        text: JSON.stringify(validFolder),
      }),
    });
    const valid = await service.selectJsonFile({ accept: ['.json', 'application/json'], maxBytes: 10 * 1024 * 1024 });
    assert.deepEqual(valid, {
      ok: true,
      value: { name: 'valid-single-folder.json', size: 321, data: validFolder },
    });
    assert.equal(state.picker.accept, '.json,application/json');

    state = installPickerDocument({ file: undefined });
    assert.deepEqual(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), { ok: true, value: null });

    state = installPickerDocument({ action: 'cancel' });
    assert.deepEqual(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), { ok: true, value: null });

    state = installPickerDocument({
      file: makeFile({ name: 'large.json', size: 11, text: JSON.stringify(validFolder) }),
    });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '文件不能超过 10 MB');

    state = installPickerDocument({
      file: makeFile({ name: 'invalid.json', size: 10, text: '{broken' }),
    });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '文件不是有效 JSON');

    // Legacy compatibility: file reads and JSON parsing share the same user-visible error.
    state = installPickerDocument({
      file: makeFile({ name: 'read-error.json', size: 10, error: true, errorValue: new Error('文件读取失败') }),
    });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '文件不是有效 JSON');

    state = installPickerDocument({
      file: makeFile({ name: 'read-non-error.json', size: 10, error: true, errorValue: '非 Error 读取异常' }),
    });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '文件不是有效 JSON');

    installPickerDocument({ hasClickError: true, clickError: new Error('文件选择器失败') });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '文件选择器失败');

    installPickerDocument({ hasClickError: true, clickError: '非 Error 选择器异常' });
    assertFailure(await service.selectJsonFile({ accept: ['.json'], maxBytes: 10 }), '收藏夹操作失败');

    let createdBlob;
    let createdUrl;
    let revokedUrl;
    let timer;
    let link;
    globalThis.Blob = class FakeBlob {
      constructor(parts, options) {
        this.parts = parts;
        this.type = options.type;
        createdBlob = this;
      }
    };
    globalThis.URL = {
      createObjectURL(value) {
        assert.equal(value, createdBlob);
        createdUrl = 'blob:synthetic-favorites';
        return createdUrl;
      },
      revokeObjectURL(value) {
        revokedUrl = value;
      },
    };
    globalThis.setTimeout = (callback, delay) => {
      timer = { callback, delay };
      return 1;
    };
    globalThis.document = {
      createElement(tag) {
        assert.equal(tag, 'a');
        link = {
          href: '',
          download: '',
          clicks: 0,
          click() { this.clicks += 1; },
        };
        return link;
      },
    };
    const saved = await service.saveJsonFile({ suggestedName: 'favorites.json', data: validFolder });
    assert.deepEqual(saved, { ok: true, value: undefined });
    assert.equal(createdBlob.type, 'application/json');
    assert.equal(createdBlob.parts[0], JSON.stringify(validFolder, null, 2));
    assert.equal(link.download, 'favorites.json');
    assert.equal(link.href, createdUrl);
    assert.equal(link.clicks, 1);
    assert.equal(timer.delay, 1000);
    timer.callback();
    assert.equal(revokedUrl, createdUrl);

    globalThis.document = {
      createElement() {
        return { click() { throw new Error('下载路径失败'); } };
      },
    };
    assertFailure(await service.saveJsonFile({ suggestedName: 'favorites.json', data: validFolder }), '下载路径失败');
  } finally {
    restoreBrowserGlobals(originals);
  }
});
