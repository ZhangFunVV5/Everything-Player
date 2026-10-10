import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildQueue,
  canGoNext,
  canGoPrevious,
  createPlayerState,
  cycleMode,
  getAdjacentIndex,
  replaceFavoriteQueue,
  replaceQueue,
  selectQueueIndex,
  updateCurrentQueueItem,
} from '../build/player-state-test/player-state.js';
import { runVersionedResolve } from '../build/player-state-test/services/versioned-resolve.js';
import { parseMp4SegmentIndex, segmentAtTime } from '../build/player-state-test/platform/mp4-segment-index.js';

test('MP4 sidx maps seek time to a fragment boundary and requires initialization bytes', () => {
  const bytes = new Uint8Array(80);
  const view = new DataView(bytes.buffer);
  const box = (offset, size, type) => {
    view.setUint32(offset, size);
    for (let index = 0; index < 4; index += 1) bytes[offset + 4 + index] = type.charCodeAt(index);
  };
  box(0, 8, 'ftyp');
  box(8, 8, 'moov');
  box(16, 56, 'sidx');
  view.setUint32(32, 1000); // timescale
  view.setUint16(46, 2); // reference count
  view.setUint32(48, 100);
  view.setUint32(52, 5000);
  view.setUint32(60, 120);
  view.setUint32(64, 5000);
  box(72, 8, 'moof');
  assert.equal(parseMp4SegmentIndex(bytes.subarray(0, 50)), null);
  const index = parseMp4SegmentIndex(bytes);
  assert.ok(index);
  assert.equal(index.initialization.length, 72);
  assert.equal(segmentAtTime(index, 2)?.byteOffset, 72);
  assert.equal(segmentAtTime(index, 7)?.byteOffset, 172);
  assert.equal(segmentAtTime(index, 11), null);
});

function deferred() {
  let resolve;
  const promise = new Promise((settle) => { resolve = settle; });
  return { promise, resolve };
}

function createResolveHarness() {
  let requestVersion = 0;
  const state = {
    queue: [{ bvid: 'BV1111111111', title: '队列快照' }],
    currentIndex: 0,
    randomHistory: [0],
    randomCursor: 0,
    currentVideo: null,
    loading: false,
    error: '',
    restoreCalls: 0,
  };

  function run(resultPromise) {
    const version = ++requestVersion;
    return runVersionedResolve({
      resolve: () => resultPromise,
      isCurrent: () => version === requestVersion,
      snapshot: () => ({
        currentIndex: state.currentIndex,
        randomHistory: [...state.randomHistory],
        randomCursor: state.randomCursor,
      }),
      start: () => {
        state.loading = true;
        state.error = '';
      },
      succeed: (value) => {
        state.queue = [{ bvid: value.bvid, title: value.title }];
        state.currentIndex = 0;
        state.currentVideo = value;
        state.loading = false;
      },
      restore: (snapshot) => {
        state.restoreCalls += 1;
        state.currentIndex = snapshot.currentIndex;
        state.randomHistory = snapshot.randomHistory;
        state.randomCursor = snapshot.randomCursor;
      },
      fail: (message) => {
        state.error = message;
        state.loading = false;
      },
    });
  }

  return { state, run };
}

const resolved = {
  bvid: 'BV2222222222',
  title: '第二集',
  duration: 122,
  coverUrl: 'cover-2',
  collection: {
    title: '测试合集',
    sections: [
      {
        title: '正片',
        episodes: [
          { bvid: 'BV1111111111', title: '第一集', duration: 61 },
          { bvid: 'BV2222222222', title: '旧标题', duration: 120 },
          { bvid: 'bv2222222222', title: '大小写重复', duration: 121 },
        ],
      },
      {
        title: '番外',
        episodes: [
          { bvid: 'BV3333333333', title: '第三集', duration: 183 },
        ],
      },
    ],
  },
};

test('buildQueue preserves collection order, deduplicates, and updates current item', () => {
  const queue = buildQueue(resolved);
  assert.equal(queue.title, '测试合集');
  assert.deepEqual(queue.items.map((item) => item.bvid), [
    'BV1111111111',
    'BV2222222222',
    'BV3333333333',
  ]);
  assert.equal(queue.currentIndex, 1);
  assert.equal(queue.items[1].title, '第二集');
  assert.equal(queue.items[1].coverUrl, 'cover-2');
});

test('sequence and repeat modes implement boundary behavior', () => {
  const state = createPlayerState();
  replaceQueue(state, resolved);
  assert.equal(getAdjacentIndex(state, 'previous'), 0);
  assert.equal(getAdjacentIndex(state, 'next'), 2);
  assert.equal(selectQueueIndex(state, 2), true);
  assert.equal(canGoNext(state), false);
  assert.equal(getAdjacentIndex(state, 'next'), null);

  cycleMode(state);
  cycleMode(state);
  assert.equal(state.mode, 'repeat');
  assert.equal(getAdjacentIndex(state, 'next'), 0);
  assert.equal(getAdjacentIndex(state, 'previous'), 1);
});

test('shuffle avoids the current item and previous follows history', () => {
  const state = createPlayerState();
  replaceQueue(state, resolved);
  cycleMode(state);
  assert.equal(state.mode, 'shuffle');
  assert.equal(canGoPrevious(state), false);
  const next = getAdjacentIndex(state, 'next', () => 0);
  assert.equal(next, 0);
  assert.equal(selectQueueIndex(state, next), true);
  assert.equal(canGoPrevious(state), true);
  assert.equal(getAdjacentIndex(state, 'previous'), 1);
});

test('selectQueueIndex updates and truncates shuffle history', () => {
  const state = createPlayerState();
  replaceQueue(state, resolved);
  cycleMode(state);
  assert.equal(selectQueueIndex(state, 0), true);
  assert.deepEqual(state.randomHistory, [1, 0]);
  assert.equal(getAdjacentIndex(state, 'next', () => 0.999999999), 2);
  assert.equal(selectQueueIndex(state, 2), true);
  assert.deepEqual(state.randomHistory, [1, 0, 2]);
  assert.equal(getAdjacentIndex(state, 'previous'), 0);
  assert.equal(selectQueueIndex(state, 1), true);
  assert.deepEqual(state.randomHistory, [1, 0, 1]);
});

test('single item queues disable previous and next navigation', () => {
  const state = createPlayerState();
  replaceQueue(state, { bvid: 'BV1111111111', title: '单集', duration: 10 });
  assert.equal(canGoPrevious(state), false);
  assert.equal(canGoNext(state), false);
  assert.equal(getAdjacentIndex(state, 'next'), null);
});

test('updateCurrentQueueItem updates metadata and reports missing items', () => {
  const state = createPlayerState();
  replaceQueue(state, resolved);
  assert.equal(updateCurrentQueueItem(state, {
    ...resolved,
    title: '刷新后的第二集',
    duration: null,
    coverUrl: 'cover-refreshed',
  }), true);
  assert.equal(state.queue[1].title, '刷新后的第二集');
  assert.equal(state.queue[1].duration, 122);
  assert.equal(state.queue[1].coverUrl, 'cover-refreshed');
  assert.equal(updateCurrentQueueItem(state, {
    ...resolved,
    bvid: 'BV9999999999',
  }), false);
});

test('favorite queue is a snapshot and remains independent of another collection', () => {
  const folder = {
    schemaVersion: 1,
    id: 'folder-id',
    name: '收藏',
    description: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    items: [
      { bvid: 'BV1111111111', title: '收藏一', owner: 'UP', duration: 10, coverUrl: 'one', addedAt: '2026-01-01T00:00:00.000Z' },
      { bvid: 'BV2222222222', title: '收藏二', owner: 'UP', duration: 20, coverUrl: 'two', addedAt: '2026-01-01T00:00:00.000Z' },
    ],
  };
  const state = createPlayerState();
  replaceFavoriteQueue(state, folder, 'BV2222222222');
  folder.items[0].title = '修改后的收藏一';
  assert.deepEqual(state.queue.map((item) => item.title), ['收藏一', '收藏二']);
  assert.equal(state.queue[0].sectionTitle, '');

  assert.equal(updateCurrentQueueItem(state, {
    ...resolved,
    bvid: 'BV2222222222',
    title: '解析后的收藏二',
    collection: { title: '另一个合集', sections: [] },
  }), true);
  assert.equal(state.queue.length, 2);
  assert.equal(state.queue[1].title, '解析后的收藏二');
  assert.equal(getAdjacentIndex(state, 'previous'), 0);
});

test('requestVersion suppresses stale success after a newer request succeeds', async () => {
  const harness = createResolveHarness();
  const oldRequest = deferred();
  const newRequest = deferred();
  const oldRun = harness.run(oldRequest.promise);
  const newRun = harness.run(newRequest.promise);

  const newest = { bvid: 'BV2222222222', title: '新请求', audioUrl: 'new' };
  newRequest.resolve({ ok: true, value: newest });
  assert.equal(await newRun, true);
  assert.equal(harness.state.currentVideo, newest);
  assert.equal(harness.state.loading, false);

  oldRequest.resolve({
    ok: true,
    value: { bvid: 'BV1111111111', title: '旧请求', audioUrl: 'old' },
  });
  assert.equal(await oldRun, false);
  assert.equal(harness.state.currentVideo, newest);
  assert.deepEqual(harness.state.queue, [{ bvid: newest.bvid, title: newest.title }]);
  assert.equal(harness.state.error, '');
  assert.equal(harness.state.restoreCalls, 0);
});

test('requestVersion suppresses stale failure without rollback or old error', async () => {
  const harness = createResolveHarness();
  const oldRequest = deferred();
  const newRequest = deferred();
  const oldRun = harness.run(oldRequest.promise);
  const newRun = harness.run(newRequest.promise);

  const newest = { bvid: 'BV3333333333', title: '保留的新状态', audioUrl: 'new' };
  newRequest.resolve({ ok: true, value: newest });
  assert.equal(await newRun, true);
  oldRequest.resolve({ ok: false, error: { message: '不应显示的旧错误' } });
  assert.equal(await oldRun, false);

  assert.equal(harness.state.currentVideo, newest);
  assert.deepEqual(harness.state.queue, [{ bvid: newest.bvid, title: newest.title }]);
  assert.equal(harness.state.loading, false);
  assert.equal(harness.state.error, '');
  assert.equal(harness.state.restoreCalls, 0);
});

test('current request failure restores its snapshot and clears loading with its error', async () => {
  const harness = createResolveHarness();
  const queueSnapshot = structuredClone(harness.state.queue);
  const request = deferred();
  const run = harness.run(request.promise);
  assert.equal(harness.state.loading, true);

  harness.state.currentIndex = 4;
  harness.state.randomHistory = [0, 4];
  harness.state.randomCursor = 1;
  request.resolve({ ok: false, error: { message: '当前请求失败' } });

  assert.equal(await run, false);
  assert.deepEqual(harness.state.queue, queueSnapshot);
  assert.equal(harness.state.currentIndex, 0);
  assert.deepEqual(harness.state.randomHistory, [0]);
  assert.equal(harness.state.randomCursor, 0);
  assert.equal(harness.state.loading, false);
  assert.equal(harness.state.error, '当前请求失败');
  assert.equal(harness.state.restoreCalls, 1);
});
