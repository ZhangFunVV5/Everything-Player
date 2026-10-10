import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import test from 'node:test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { createDemoServer } from '../referenceCODE/server.mjs';
import { FavoritesStore } from '../referenceCODE/favorites-store.mjs';
import { LegacyHttpFavoritesRepository } from '../build/adapter-contract-test/repositories/legacy-http-favorites-repository.js';

const FIXTURE_DIR = new URL('../referenceCODE/test/fixtures/favorites/', import.meta.url);
const VALID_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f-]{27}$/;

async function readFixture(name) {
  return JSON.parse(await readFile(new URL(name, FIXTURE_DIR), 'utf8'));
}

async function directoryEntryCount(directory) {
  try {
    return (await readdir(directory)).length;
  } catch (error) {
    if (error?.code === 'ENOENT') return 0;
    throw error;
  }
}

function unwrap(result) {
  assert.equal(result.ok, true, result.ok ? '' : result.error.message);
  return result.value;
}

function assertFailure(result, message) {
  assert.deepEqual(result, { ok: false, error: { message } });
}

function assertGeneratedId(id) {
  assert.equal(typeof id, 'string');
  assert.match(id, VALID_ID_PATTERN);
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  return `http://127.0.0.1:${server.address().port}/`;
}

async function close(server) {
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}

test('P5-T6 runs the common schema and repository cases against the real legacy server', { concurrency: false }, async () => {
  const valid = await readFixture('valid-single-folder.json');
  const multi = await readFixture('multi-folder.json');
  const duplicates = await readFixture('duplicates.json');
  const partial = await readFixture('partial-import.json');
  const directory = await mkdtemp(join(tmpdir(), 'everything-player-p5-t6-legacy-'));
  const originalFetch = globalThis.fetch;
  let baseUrl;
  let server;

  const metadataResolver = async (input) => ({
    bvid: input,
    title: `Resolved ${input}`,
    owner: 'Synthetic resolver',
    duration: 42,
    coverUrl: 'https://example.com/resolved.jpg',
  });

  try {
    globalThis.fetch = (input, options) => originalFetch(
      typeof input === 'string' ? new URL(input, baseUrl) : input,
      options,
    );
    server = createDemoServer({ favoritesDirectory: directory, metadataResolver });
    baseUrl = await listen(server);
    let repository = new LegacyHttpFavoritesRepository();

    assert.deepEqual(unwrap(await repository.list()), { folders: [], errors: [] });

    const beforePreview = await directoryEntryCount(directory);
    const preview = unwrap(await repository.previewImport(multi));
    assert.equal(preview.preview, true);
    assert.deepEqual(preview.folders, multi.folders);
    assert.equal(await directoryEntryCount(directory), beforePreview);

    const importedSingle = unwrap(await repository.commitImport(valid));
    assert.equal(importedSingle.error, undefined);
    assert.equal(importedSingle.folders.length, 1);
    assertGeneratedId(importedSingle.folders[0].id);
    assert.notEqual(importedSingle.folders[0].id, valid.id);
    assert.deepEqual({ ...importedSingle.folders[0], id: valid.id }, valid);

    const importedMulti = unwrap(await repository.commitImport(multi));
    assert.equal(importedMulti.folders.length, 2);
    assert.deepEqual(
      importedMulti.folders.map((folder) => folder.name),
      multi.folders.map((folder) => folder.name),
    );
    importedMulti.folders.forEach((folder, index) => {
      assertGeneratedId(folder.id);
      assert.notEqual(folder.id, multi.folders[index].id);
      assert.deepEqual({ ...folder, id: multi.folders[index].id }, multi.folders[index]);
    });

    const importedDuplicates = unwrap(await repository.commitImport(duplicates));
    assert.equal(importedDuplicates.folders[0].items.length, 2);
    assert.deepEqual(
      importedDuplicates.folders[0].items.map((item) => item.bvid),
      ['BV1D2U3P4L5M', 'BV1d2U3P4L5M'],
    );

    const secondImport = unwrap(await repository.commitImport(valid));
    assert.equal(secondImport.folders.length, 1);
    assert.notEqual(secondImport.folders[0].id, importedSingle.folders[0].id);
    assert.equal(unwrap(await repository.list()).folders.length, 5);

    const created = unwrap(await repository.create({
      name: 'Synthetic command folder',
      description: 'Created through the real HTTP route',
    }));
    assertGeneratedId(created.id);
    const resolvedBvid = 'BV1A2B3C4D5E';
    const added = unwrap(await repository.addItem(created.id, { input: resolvedBvid }));
    assert.equal(added.duplicate, false);
    assert.equal(added.folder.items[0].bvid, resolvedBvid);
    const duplicate = unwrap(await repository.addItem(created.id, { input: resolvedBvid }));
    assert.equal(duplicate.duplicate, true);
    assert.deepEqual(duplicate.folder, added.folder);
    assert.deepEqual(unwrap(await repository.get(created.id)), added.folder);

    const concurrent = unwrap(await repository.create({
      name: 'Synthetic concurrent folder',
      description: '',
    }));
    const concurrentInputs = Array.from({ length: 8 }, (_, index) => `BV${String(index).padStart(10, '0')}`);
    const concurrentResults = await Promise.all(
      concurrentInputs.map((input) => repository.addItem(concurrent.id, { input })),
    );
    concurrentResults.forEach((result) => {
      const value = unwrap(result);
      assert.equal(value.duplicate, false);
    });
    const concurrentLoaded = unwrap(await repository.get(concurrent.id));
    assert.deepEqual(
      new Set(concurrentLoaded.items.map((item) => item.bvid)),
      new Set(concurrentInputs),
    );
    assert.equal(concurrentLoaded.items.length, concurrentInputs.length);

    const listed = unwrap(await repository.list());
    assert.equal(listed.errors.length, 0);
    const updatedTimes = listed.folders.map((folder) => Date.parse(folder.updatedAt));
    assert.deepEqual(updatedTimes, [...updatedTimes].sort((left, right) => right - left));
    assert.deepEqual(unwrap(await repository.exportFolder(created.id)), added.folder);
    const exported = unwrap(await repository.exportAll());
    assert.deepEqual(exported, {
      schemaVersion: 1,
      type: 'bili-sound-favorites',
      folders: listed.folders,
    });

    const corruptName = '99999999-9999-4999-8999-999999999999.json';
    const corruptPath = join(directory, corruptName);
    const corruptBytes = '{broken';
    await writeFile(corruptPath, corruptBytes);
    const withCorrupt = unwrap(await repository.list());
    assert.equal(withCorrupt.folders.length, listed.folders.length);
    assert.ok(withCorrupt.errors.some((error) => error.startsWith(`${corruptName}：`)));
    assert.equal(await readFile(corruptPath, 'utf8'), corruptBytes);
    const exportFailure = await repository.exportAll();
    assert.equal(exportFailure.ok, false);
    assert.match(exportFailure.error.message, new RegExp(`^${corruptName}：`));

    assertFailure(
      await repository.previewImport({ schemaVersion: 2 }),
      '导入格式或版本无效',
    );
    assertFailure(
      await repository.get('not-an-id'),
      '收藏夹 ID 无效',
    );
    assertFailure(
      await repository.get('88888888-8888-4888-8888-888888888888'),
      '收藏夹不存在',
    );

    await close(server);
    server = createDemoServer({ favoritesDirectory: directory, metadataResolver });
    baseUrl = await listen(server);
    repository = new LegacyHttpFavoritesRepository();
    const restarted = unwrap(await repository.list());
    assert.equal(restarted.folders.length, listed.folders.length);
    assert.ok(restarted.errors.some((error) => error.startsWith(`${corruptName}：`)));
    assert.deepEqual(unwrap(await repository.get(created.id)), added.folder);
  } finally {
    globalThis.fetch = originalFetch;
    if (server) await close(server);
    await rm(directory, { recursive: true, force: true });
  }
});

test('P5-T6 reuses the real legacy store partial-write contract in an isolated directory', async () => {
  const partial = await readFixture('partial-import.json');
  const directory = await mkdtemp(join(tmpdir(), 'everything-player-p5-t6-partial-'));
  try {
    const store = new FavoritesStore(directory);
    const save = store.save.bind(store);
    let writes = 0;
    store.save = async (folder) => {
      writes += 1;
      if (writes === 2) throw new Error('disk full');
      return save(folder);
    };

    const result = await store.import(partial);
    assert.equal(result.folders.length, 1);
    assert.match(result.error, /^已导入 1 个收藏夹，后续写入失败：disk full$/);
    assert.equal((await store.list()).folders.length, 1);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
