import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { FavoritesStore } from '../favorites-store.mjs';
import { createDemoServer } from '../server.mjs';
import { createPlayerState, replaceFavoriteQueue, updateCurrentQueueItem, getAdjacentIndex } from '../public/player-state.js';

const video = n => ({ bvid: `BV${String(n).padStart(10, '0')}`, title: `视频 ${n}`, owner: 'UP', duration: 60, coverUrl: `https://example.com/${n}.jpg` });
async function setup(t) {
  const directory = await mkdtemp(join(tmpdir(), 'bili-favorites-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return { directory, store: new FavoritesStore(directory) };
}
test('concurrent adds persist across restart; duplicate keeps time and newest cover', async t => {
  const { directory, store } = await setup(t);
  const folder = await store.create('收藏', '描述');
  await Promise.all([store.add(folder.id, video(1)), store.add(folder.id, video(2))]);
  const loaded = await new FavoritesStore(directory).get(folder.id);
  assert.deepEqual(loaded.items.map(v => v.bvid), [video(2).bvid, video(1).bvid]);
  assert.equal(loaded.items[0].coverUrl, video(2).coverUrl);
  const duplicate = await store.add(folder.id, video(1));
  assert.equal(duplicate.duplicate, true);
  assert.deepEqual(duplicate.folder, loaded);
  assert.equal(JSON.parse(await readFile(join(directory, `${folder.id}.json`))).items.length, 2);
});
test('import preview does not write; roundtrip creates new ID and validates all folders first', async t => {
  const { directory, store } = await setup(t);
  const original = await store.create('同名', ''); await store.add(original.id, video(1));
  const folder = await store.get(original.id);
  const data = { schemaVersion: 1, type: 'bili-sound-favorites', folders: [folder] };
  await store.import(data, true); assert.equal((await readdir(directory)).length, 1);
  const imported = (await store.import(data)).folders[0];
  assert.notEqual(imported.id, original.id); assert.deepEqual(imported.items, folder.items);
  await assert.rejects(store.import({ ...data, folders: [folder, { ...folder, schemaVersion: 99 }] }));
  assert.equal((await readdir(directory)).length, 2);
  await assert.rejects(store.import({ ...folder, items: [{ ...folder.items[0], coverUrl: 'javascript:alert(1)' }] }));
});
test('corrupt file remains intact; disk failures do not report success', async t => {
  const { directory, store } = await setup(t);
  const folder = await store.create('有效', '');
  await writeFile(join(directory, `${folder.id}.json`), '{broken');
  assert.equal((await store.list()).errors.length, 1);
  await assert.rejects(store.add(folder.id, video(1)));
  assert.equal(await readFile(join(directory, `${folder.id}.json`), 'utf8'), '{broken');
  const blocked = join(directory, 'file'); await writeFile(blocked, 'not directory');
  await assert.rejects(new FavoritesStore(blocked).create('失败', ''));
});
test('API create, add, export, import and restart retain favorites', async t => {
  const { directory } = await setup(t);
  const server = createDemoServer({ favoritesDirectory: directory, metadataResolver: async () => video(1) });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
  const base = `http://127.0.0.1:${server.address().port}/api/favorites`;
  const post = async (path, body) => { const r = await fetch(base + path, { method: 'POST', body: JSON.stringify(body) }); return { status: r.status, body: await r.json() }; };
  const created = await post('', { name: '接口收藏', description: '' }); assert.equal(created.status, 201);
  assert.equal((await post(`/${created.body.id}/items`, { input: video(1).bvid })).status, 200);
  const exported = await (await fetch(base + '/export')).json();
  assert.equal((await post('/import', { data: exported })).body.folders.length, 1);
  assert.equal((await new FavoritesStore(directory).list()).folders.length, 2);
  assert.equal((await post('', { name: '', description: '' })).status, 400);
  assert.equal((await fetch(base + '/unknown/path/export')).status, 404);
  assert.equal((await fetch(base + `/${created.body.id}/export`)).status, 200);
  const malformed = await fetch(base + '/import', { method: 'POST', body: '{broken' });
  assert.equal(malformed.status, 400);
});

test('import deduplicates exact BV values and reports partial disk failure', async t => {
  const { store } = await setup(t);
  const folder = await store.create('导入测试', '');
  const addedAt = new Date().toISOString();
  const first = { ...video(1), bvid: 'BV1xx411c7mD', addedAt };
  const second = { ...first, bvid: 'BV1Xx411c7mD' };
  folder.items = [first, { ...first, title: '重复' }, second];
  const preview = await store.import(folder, true);
  assert.deepEqual(preview.folders[0].items, [first, second]);
  await assert.rejects(store.import({ ...folder, createdAt: 'not-a-date' }));
  const save = store.save.bind(store); let writes = 0;
  store.save = async value => { if (++writes === 2) throw new Error('disk full'); return save(value); };
  const result = await store.import({ schemaVersion: 1, type: 'bili-sound-favorites', folders: [folder, folder] });
  assert.equal(result.folders.length, 1);
  assert.match(result.error, /已导入 1.*disk full/);
  assert.equal((await store.list()).folders.length, 2);
});
test('favorite queue survives video metadata containing a different Bilibili collection', () => {
  const state = createPlayerState(); const folder = { id: 'folder', name: '收藏', items: [video(1), video(2)] };
  replaceFavoriteQueue(state, folder, video(1).bvid);
  updateCurrentQueueItem(state, { ...video(2), collection: { title: '其他合集' } });
  assert.equal(state.sourceType, 'favorite'); assert.equal(state.queueTitle, '收藏');
  assert.equal(state.currentIndex, 1); assert.equal(getAdjacentIndex(state, 'previous'), 0);
  folder.items.push(video(3)); assert.equal(state.queue.length, 2);
});
