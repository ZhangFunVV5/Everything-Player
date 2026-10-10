import assert from 'node:assert/strict';
import { request as httpRequest } from 'node:http';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  createDemoServer,
  fetchVideoMetadata,
} from '../server.mjs';
import { FavoritesStore } from '../favorites-store.mjs';
import { createOversizedImportBody, OVERSIZED_MIN_BYTES } from '../fixtures/favorites/oversized-import.mjs';

const FIXTURE_DIR = new URL('./fixtures/favorites/', import.meta.url);

async function readFixture(name) {
  return JSON.parse(await readFile(new URL(name, FIXTURE_DIR), 'utf8'));
}

async function readRawFixture(name) {
  return readFile(new URL(name, FIXTURE_DIR), 'utf8');
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function jsonResponse(data, status = 200, headers = {}) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

async function startDemo(t, options = {}) {
  const ownsDirectory = !options.favoritesDirectory;
  const directory = options.favoritesDirectory || await mkdtemp(join(tmpdir(), 'bili-contract-'));
  const server = createDemoServer({ ...options, favoritesDirectory: directory });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve, reject) => {
      server.close(error => error && error.code !== 'ERR_SERVER_NOT_RUNNING' ? reject(error) : resolve());
    });
    if (ownsDirectory) await rm(directory, { recursive: true, force: true });
  });
  return { directory, server };
}

function requestLocal(server, { method = 'GET', path = '/', headers = {}, body } = {}) {
  const address = server.address();
  assert(address && typeof address === 'object' && address.port);
  const payload = body === undefined || body === null ? null : Buffer.from(body);
  const requestHeaders = { ...headers };
  if (payload !== null) {
    requestHeaders['Content-Length'] ??= payload.byteLength;
    requestHeaders['Content-Type'] ??= 'application/json';
  }
  return new Promise((resolve, reject) => {
    const request = httpRequest({
      hostname: '127.0.0.1',
      port: address.port,
      path,
      method,
      headers: requestHeaders,
    }, response => {
      const chunks = [];
      response.on('data', chunk => chunks.push(chunk));
      response.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        let json = null;
        try { json = text ? JSON.parse(text) : null; } catch {}
        resolve({
          status: response.statusCode,
          headers: response.headers,
          body: text,
          json,
        });
      });
    });
    request.on('error', reject);
    if (payload !== null) request.end(payload);
    else request.end();
  });
}

function makeResolveView() {
  return {
    bvid: 'BV1A2B3C4D5E',
    title: 'Synthetic Resolve Video',
    owner: { name: 'Synthetic Resolve UP' },
    pic: 'http://example.com/resolve-cover.jpg',
    duration: 999,
    cid: 9099,
    pages: [
      { cid: 9101, part: 'Synthetic First Page', duration: 111 },
      { cid: 9102, part: 'Synthetic Second Page', duration: 222 },
    ],
    ugc_season: {
      id: 901,
      title: 'Synthetic Collection',
      sections: [
        {
          id: 1,
          title: 'Synthetic Section One',
          episodes: [
            {
              bvid: 'BV1F2G3H4J5K',
              title: 'Synthetic Episode One',
              page: { duration: 31 },
              arc: { pic: 'http://example.com/episode-one.jpg' },
            },
            {
              bvid: 'BV1N2O3P4Q5R',
              arc: { pic: 'https://example.com/episode-two.jpg' },
            },
          ],
        },
        {
          id: 2,
          title: 'Synthetic Section Two',
          episodes: [
            {
              bvid: 'BV1S2T3U4V5W',
              title: 'Synthetic Episode Three',
              page: { duration: 33 },
              arc: { pic: 'https://example.com/episode-three.jpg' },
            },
          ],
        },
      ],
    },
  };
}

function makeResolveFetch(state) {
  return async (input, options = {}) => {
    const url = new URL(String(input));
    state.calls.push({ url, options });
    if (url.pathname === '/x/web-interface/view') {
      if (state.viewMode === 'nonzero') return jsonResponse({ code: -400, message: 'synthetic view error', data: null });
      if (state.viewMode === 'missing-cid') {
        const data = makeResolveView();
        data.pages = [];
        data.cid = null;
        return jsonResponse({ code: 0, data });
      }
      return jsonResponse({ code: 0, data: makeResolveView() });
    }
    if (url.pathname === '/x/player/playurl') {
      if (state.playMode === 'no-audio') return jsonResponse({ code: 0, data: { dash: { audio: [] } } });
      if (state.playMode === 'nonzero') return jsonResponse({ code: -403, message: 'synthetic play error', data: null });
      return jsonResponse({
        code: 0,
        data: {
          dash: {
            audio: [
              { baseUrl: 'https://cdn.example.com/low-audio', bandwidth: 64000 },
              { baseUrl: 'https://cdn.example.com/high-audio', bandwidth: 192000 },
            ],
            dolby: { audio: [{ baseUrl: 'https://cdn.example.com/top-audio', bandwidth: 256000 }] },
          },
        },
      });
    }
    throw new Error(`unexpected external URL: ${url}`);
  };
}

test('resolve contract uses mocked Bilibili metadata and play-url responses', { concurrency: false }, async t => {
  const state = { viewMode: 'success', playMode: 'success', calls: [] };
  const originalFetch = globalThis.fetch;
  const { server } = await startDemo(t);
  globalThis.fetch = makeResolveFetch(state);
  try {
    const direct = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'BV1A2B3C4D5E' }),
    });
    assert.equal(direct.status, 200);
    assert.equal(direct.json.bvid, 'BV1A2B3C4D5E');
    assert.equal(direct.json.cid, 9101);
    assert.equal(direct.json.title, 'Synthetic Resolve Video');
    assert.equal(direct.json.owner, 'Synthetic Resolve UP');
    assert.equal(direct.json.pageTitle, 'Synthetic First Page');
    assert.equal(direct.json.duration, 111);
    assert.equal(direct.json.coverUrl, 'https://example.com/resolve-cover.jpg');
    assert.equal(direct.json.collection.id, 901);
    assert.deepEqual(direct.json.collection.sections.map(section => section.title), [
      'Synthetic Section One',
      'Synthetic Section Two',
    ]);
    assert.deepEqual(direct.json.collection.sections[0].episodes.map(episode => episode.bvid), [
      'BV1F2G3H4J5K',
      'BV1N2O3P4Q5R',
    ]);
    assert.equal(direct.json.collection.sections[0].episodes[1].duration, null);
    assert.match(direct.json.audioUrl, /^\/api\/audio\/[0-9a-f-]{36}$/i);
    assert(!direct.json.audioUrl.includes('cdn.example.com'));

    const fullUrl = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'https://www.bilibili.com/video/BV1A2B3C4D5E?p=2' }),
    });
    assert.equal(fullUrl.status, 200);
    assert.equal(fullUrl.json.bvid, 'BV1A2B3C4D5E');

    const viewCall = state.calls.find(call => call.url.pathname === '/x/web-interface/view');
    const playCall = state.calls.find(call => call.url.pathname === '/x/player/playurl');
    assert.equal(viewCall.url.searchParams.get('bvid'), 'BV1A2B3C4D5E');
    assert.equal(viewCall.options.headers.Referer, 'https://www.bilibili.com/');
    assert.match(viewCall.options.headers['User-Agent'], /^Mozilla\/5\.0/);
    assert.equal(playCall.url.searchParams.get('cid'), '9101');
    assert.equal(playCall.url.searchParams.get('fnval'), '16');
    assert.equal(playCall.url.searchParams.get('fnver'), '0');
    assert.equal(playCall.url.searchParams.get('fourk'), '1');

    const invalid = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'not-a-bv' }),
    });
    assert.equal(invalid.status, 400);
    assert.equal(invalid.json.error, '请输入有效的 BV 号或 Bilibili 视频链接');

    state.viewMode = 'nonzero';
    const upstreamError = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'BV1A2B3C4D5E' }),
    });
    assert.equal(upstreamError.status, 400);
    assert.equal(upstreamError.json.error, 'synthetic view error');

    state.viewMode = 'missing-cid';
    const missingCid = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'BV1A2B3C4D5E' }),
    });
    assert.equal(missingCid.status, 400);
    assert.equal(missingCid.json.error, '未找到视频分 P 信息');

    state.viewMode = 'success';
    state.playMode = 'no-audio';
    const noAudio = await requestLocal(server, {
      method: 'POST',
      path: '/api/resolve',
      body: JSON.stringify({ input: 'BV1A2B3C4D5E' }),
    });
    assert.equal(noAudio.status, 400);
    assert.equal(noAudio.json.error, '该视频没有可用的 DASH 音轨');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('favorites contract uses schema-v1 fixtures and isolated directories', { concurrency: false }, async t => {
  const valid = await readFixture('valid-single-folder.json');
  const duplicates = await readFixture('duplicates.json');
  const multi = await readFixture('multi-folder.json');
  const partial = await readFixture('partial-import.json');
  const corruptJson = await readRawFixture('corrupt-json.json');
  const metadata = { ...valid.items[0] };
  const { server, directory } = await startDemo(t, {
    metadataResolver: async input => {
      assert.equal(input, metadata.bvid);
      return metadata;
    },
  });

  const create = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites',
    body: JSON.stringify({ name: 'Contract Folder', description: 'Synthetic contract test' }),
  });
  assert.equal(create.status, 201);
  assert.match(create.json.id, /^[0-9a-f-]{36}$/i);
  assert.equal(create.json.items.length, 0);

  const listedEmpty = await requestLocal(server, { path: '/api/favorites' });
  assert.equal(listedEmpty.status, 200);
  assert.equal(listedEmpty.json.folders.length, 1);
  assert.deepEqual(listedEmpty.json.errors, []);

  const folderId = create.json.id;
  const got = await requestLocal(server, { path: `/api/favorites/${folderId}` });
  assert.equal(got.status, 200);
  assert.equal(got.json.id, folderId);

  const added = await requestLocal(server, {
    method: 'POST',
    path: `/api/favorites/${folderId}/items`,
    body: JSON.stringify({ input: metadata.bvid }),
  });
  assert.equal(added.status, 200);
  assert.equal(added.json.duplicate, false);
  assert.deepEqual(added.json.folder.items.map(item => item.bvid), [metadata.bvid]);

  const duplicate = await requestLocal(server, {
    method: 'POST',
    path: `/api/favorites/${folderId}/items`,
    body: JSON.stringify({ input: metadata.bvid }),
  });
  assert.equal(duplicate.status, 200);
  assert.equal(duplicate.json.duplicate, true);
  assert.equal(duplicate.json.folder.items.length, 1);

  const singleExport = await requestLocal(server, { path: `/api/favorites/${folderId}/export` });
  assert.equal(singleExport.status, 200);
  assert.match(singleExport.headers['content-disposition'], /attachment/);
  assert.equal(singleExport.json.schemaVersion, 1);
  assert.equal(singleExport.json.id, folderId);

  const allExport = await requestLocal(server, { path: '/api/favorites/export' });
  assert.equal(allExport.status, 200);
  assert.equal(allExport.json.type, 'bili-sound-favorites');
  assert.equal(allExport.json.folders.length, 1);

  const preview = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites/import',
    body: JSON.stringify({ data: multi, preview: true }),
  });
  assert.equal(preview.status, 200);
  assert.equal(preview.json.preview, true);
  assert.deepEqual(preview.json.folders.map(folder => folder.id), multi.folders.map(folder => folder.id));
  assert.equal((await requestLocal(server, { path: '/api/favorites' })).json.folders.length, 1);

  const imported = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites/import',
    body: JSON.stringify({ data: multi }),
  });
  assert.equal(imported.status, 200);
  assert.equal(imported.json.folders.length, 2);
  assert.notEqual(imported.json.folders[0].id, multi.folders[0].id);
  assert.notEqual(imported.json.folders[1].id, multi.folders[1].id);
  assert.notEqual(imported.json.folders[0].id, imported.json.folders[1].id);

  const duplicatePreview = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites/import',
    body: JSON.stringify({ data: duplicates, preview: true }),
  });
  assert.equal(duplicatePreview.status, 200);
  assert.deepEqual(duplicatePreview.json.folders[0].items.map(item => item.bvid), [
    'BV1D2U3P4L5M',
    'BV1d2U3P4L5M',
  ]);

  const emptyImport = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites/import',
    body: JSON.stringify({ data: { schemaVersion: 1, type: 'bili-sound-favorites', folders: [], padding: 'ignored' } }),
  });
  assert.equal(emptyImport.status, 200);
  assert.deepEqual(emptyImport.json, { folders: [] });

  const invalidSchema = clone(valid);
  invalidSchema.schemaVersion = 2;
  const schemaError = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: JSON.stringify({ data: invalidSchema }),
  });
  assert.equal(schemaError.status, 400);
  assert.equal(typeof schemaError.json.error, 'string');

  const invalidDate = clone(valid);
  invalidDate.createdAt = 'not-a-date';
  const dateError = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: JSON.stringify({ data: invalidDate }),
  });
  assert.equal(dateError.status, 400);

  const invalidCover = clone(valid);
  invalidCover.items[0].coverUrl = 'javascript:alert(1)';
  const coverError = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: JSON.stringify({ data: invalidCover }),
  });
  assert.equal(coverError.status, 400);

  const invalidDuration = clone(valid);
  invalidDuration.items[0].duration = -1;
  const durationError = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: JSON.stringify({ data: invalidDuration }),
  });
  assert.equal(durationError.status, 400);

  const nameError = await requestLocal(server, {
    method: 'POST',
    path: '/api/favorites',
    body: JSON.stringify({ name: 'x'.repeat(81), description: '' }),
  });
  assert.equal(nameError.status, 400);

  const malformed = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: '{broken',
  });
  assert.equal(malformed.status, 400);
  assert.equal(malformed.json.error, '请求内容不是有效 JSON');

  const corruptDirectory = await mkdtemp(join(tmpdir(), 'bili-contract-corrupt-'));
  const partialDirectory = await mkdtemp(join(tmpdir(), 'bili-contract-partial-'));
  t.after(() => Promise.all([
    rm(corruptDirectory, { recursive: true, force: true }),
    rm(partialDirectory, { recursive: true, force: true }),
  ]));
  const corruptId = '77777777-7777-4777-8777-777777777777';
  await writeFile(join(corruptDirectory, `${corruptId}.json`), corruptJson);
  const corruptList = await new FavoritesStore(corruptDirectory).list();
  assert.equal(corruptList.folders.length, 0);
  assert.equal(corruptList.errors.length, 1);
  assert.match(corruptList.errors[0], /JSON/);

  const partialStore = new FavoritesStore(partialDirectory);
  const save = partialStore.save.bind(partialStore);
  let writes = 0;
  partialStore.save = async value => {
    writes += 1;
    if (writes === 2) throw new Error('disk full');
    return save(value);
  };
  const partialResult = await partialStore.import(partial);
  assert.equal(partialResult.folders.length, 1);
  assert.match(partialResult.error, /已导入 1.*disk full/);
  assert.equal((await readdir(partialDirectory)).length, 1);

  assert.equal(directory.startsWith(tmpdir()), true);
});

test('audio contract forwards range metadata and controls token lifetime', { concurrency: false }, async t => {
  const definitions = new Map([
    ['BV1A2B3C4D5E', {
      primary: 'https://cdn.example.com/range-primary',
      backup: 'https://cdn.example.com/range-backup',
      status: 206,
      body: 'DATA',
      headers: {
        'Content-Type': 'application/octet-stream',
        'Content-Length': '4',
        'Content-Range': 'bytes 0-3/100',
        'Accept-Ranges': 'bytes',
        'Cache-Control': 'public, max-age=60',
        ETag: 'etag-v1',
        'Last-Modified': 'synthetic-date',
        'X-Not-Forwarded': 'secret',
      },
    }],
    ['BV1F2G3H4J5K', {
      primary: 'https://cdn.example.com/fallback-primary',
      backup: 'https://cdn.example.com/fallback-backup',
      status: 200,
      body: 'BACKUP',
      headers: { 'Content-Type': 'audio/mpeg', 'Content-Length': '6' },
      primaryFailure: true,
    }],
    ['BV1N2O3P4Q5R', {
      primary: 'https://cdn.example.com/fail-primary',
      backup: 'https://cdn.example.com/fail-backup',
      status: 200,
      body: 'NEVER',
      headers: { 'Content-Type': 'audio/mpeg' },
      primaryFailure: true,
      backupFailure: true,
    }],
    ['BV1S2T3U4V5W', {
      primary: 'https://cdn.example.com/missing-content-type',
      backup: 'https://cdn.example.com/missing-content-type-backup',
      status: 200,
      body: 'MISS',
      headers: { 'Content-Length': '4' },
    }],
  ]);
  const cdnCalls = [];
  const originalFetch = globalThis.fetch;
  const originalNow = Date.now;
  let now = 1_700_000_000_000;
  const { server, directory } = await startDemo(t);
  Date.now = () => now;
  globalThis.fetch = async (input, options = {}) => {
    const url = new URL(String(input));
    if (url.pathname === '/x/web-interface/view') {
      const bvid = url.searchParams.get('bvid');
      const definition = definitions.get(bvid);
      return jsonResponse({
        code: 0,
        data: {
          bvid,
          title: `Synthetic ${bvid}`,
          owner: { name: 'Synthetic Audio UP' },
          pic: 'https://example.com/audio-cover.jpg',
          pages: [{ cid: 7001, part: 'Synthetic Audio Page', duration: 10 }],
          ugc_season: null,
          definition,
        },
      });
    }
    if (url.pathname === '/x/player/playurl') {
      const bvid = url.searchParams.get('bvid');
      const definition = definitions.get(bvid);
      return jsonResponse({
        code: 0,
        data: { dash: { audio: [{ baseUrl: definition.primary, backupUrl: [definition.backup], bandwidth: 192000 }] } },
      });
    }
    const definition = [...definitions.values()].find(value => [value.primary, value.backup].includes(url.href));
    assert(definition, `unexpected external URL: ${url}`);
    cdnCalls.push({ url: url.href, method: options.method, headers: options.headers });
    if ((url.href === definition.primary && definition.primaryFailure) ||
        (url.href === definition.backup && definition.backupFailure)) {
      return new Response(null, { status: 503 });
    }
    return new Response(Buffer.from(definition.body), {
      status: definition.status,
      headers: definition.headers,
    });
  };
  try {
    const resolve = async bvid => requestLocal(server, {
      method: 'POST', path: '/api/resolve', body: JSON.stringify({ input: bvid }),
    });

    const rangeToken = (await resolve('BV1A2B3C4D5E')).json.audioUrl.split('/').pop();
    const head = await requestLocal(server, { method: 'HEAD', path: `/api/audio/${rangeToken}` });
    assert.equal(head.status, 206);
    assert.equal(head.body, '');
    assert.equal(head.headers['content-type'], 'audio/mp4');
    assert.equal(head.headers['content-range'], 'bytes 0-3/100');
    assert.equal(head.headers['content-length'], '4');
    assert.equal(head.headers['accept-ranges'], 'bytes');
    assert.equal(head.headers['cache-control'], 'public, max-age=60');
    assert.equal(head.headers.etag, 'etag-v1');
    assert.equal(head.headers['last-modified'], 'synthetic-date');
    assert.equal(head.headers['x-not-forwarded'], undefined);

    const ranged = await requestLocal(server, {
      method: 'GET',
      path: `/api/audio/${rangeToken}`,
      headers: { Range: 'bytes=0-3', 'If-Range': 'etag-v1' },
    });
    assert.equal(ranged.status, 206);
    assert.equal(ranged.body, 'DATA');
    const rangeCall = cdnCalls.find(call => call.url === 'https://cdn.example.com/range-primary' && call.method === 'GET');
    assert.equal(rangeCall.headers.Range, 'bytes=0-3');
    assert.equal(rangeCall.headers['If-Range'], 'etag-v1');
    assert.equal(rangeCall.headers.Accept, '*/*');

    const fallbackStart = cdnCalls.length;
    const fallbackToken = (await resolve('BV1F2G3H4J5K')).json.audioUrl.split('/').pop();
    const fallback = await requestLocal(server, { method: 'GET', path: `/api/audio/${fallbackToken}` });
    assert.equal(fallback.status, 200);
    assert.equal(fallback.body, 'BACKUP');
    assert.deepEqual(cdnCalls.slice(fallbackStart).map(call => call.url), [
      'https://cdn.example.com/fallback-primary',
      'https://cdn.example.com/fallback-backup',
    ]);

    const failedToken = (await resolve('BV1N2O3P4Q5R')).json.audioUrl.split('/').pop();
    const failed = await requestLocal(server, { method: 'GET', path: `/api/audio/${failedToken}` });
    assert.equal(failed.status, 502);
    assert.equal(failed.json.error, 'Bilibili 音频源暂时不可用，请重新解析');

    const missingTypeToken = (await resolve('BV1S2T3U4V5W')).json.audioUrl.split('/').pop();
    const missingType = await requestLocal(server, { method: 'GET', path: `/api/audio/${missingTypeToken}` });
    assert.equal(missingType.status, 200);
    assert.equal(missingType.headers['content-type'], 'audio/mp4');
    assert.equal(missingType.body, 'MISS');

    now += 30 * 60 * 1000;
    const expired = await requestLocal(server, { method: 'GET', path: `/api/audio/${rangeToken}` });
    assert.equal(expired.status, 404);
    assert.equal(expired.json.error, '播放地址已失效，请重新提交 BV 号');

    const invalidToken = await requestLocal(server, {
      method: 'GET', path: '/api/audio/00000000-0000-4000-8000-000000000000',
    });
    assert.equal(invalidToken.status, 404);
    assert.equal(invalidToken.json.error, '播放地址已失效，请重新提交 BV 号');
    assert.deepEqual(await readdir(directory), []);
  } finally {
    globalThis.fetch = originalFetch;
    Date.now = originalNow;
  }
});

test('request body limits and unsupported methods return stable errors', { concurrency: false }, async t => {
  const { server } = await startDemo(t);
  const defaultLimitBody = JSON.stringify({ name: 'x'.repeat(16 * 1024), description: '' });
  const tooLargeDefault = await requestLocal(server, {
    method: 'POST', path: '/api/favorites', body: defaultLimitBody,
  });
  assert.equal(Buffer.byteLength(defaultLimitBody), 16412);
  assert.equal(tooLargeDefault.status, 400);
  assert.equal(tooLargeDefault.json.error, '请求内容过大');

  const malformed = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: '{broken',
  });
  assert.equal(malformed.status, 400);
  assert.equal(malformed.json.error, '请求内容不是有效 JSON');

  let oversizedRequestBody = `{"data":${createOversizedImportBody()}}`;
  const oversizedBytes = Buffer.byteLength(oversizedRequestBody, 'utf8');
  assert(oversizedBytes > OVERSIZED_MIN_BYTES);
  const oversized = await requestLocal(server, {
    method: 'POST', path: '/api/favorites/import', body: oversizedRequestBody,
  });
  oversizedRequestBody = null;
  assert.equal(oversized.status, 400);
  assert.equal(oversized.json.error, '请求内容过大');

  const unsupported = await requestLocal(server, { method: 'POST', path: '/not-an-api-route', body: '' });
  assert.equal(unsupported.status, 405);
  assert.equal(unsupported.json.error, '不支持的请求方法');
});
