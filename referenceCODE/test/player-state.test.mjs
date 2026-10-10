import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildQueue,
  canGoNext,
  canGoPrevious,
  createPlayerState,
  cycleMode,
  getAdjacentIndex,
  replaceQueue,
  selectQueueIndex,
} from '../public/player-state.js';

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
          { bvid: 'BV3333333333', title: '第三集', duration: 183 },
        ],
      },
    ],
  },
};

test('buildQueue keeps collection order and updates the current item', () => {
  const queue = buildQueue(resolved);
  assert.equal(queue.title, '测试合集');
  assert.equal(queue.items.length, 3);
  assert.equal(queue.currentIndex, 1);
  assert.equal(queue.items[1].title, '第二集');
  assert.equal(queue.items[1].coverUrl, 'cover-2');
});

test('sequence and repeat modes implement boundary behavior', () => {
  const state = createPlayerState();
  replaceQueue(state, resolved);
  assert.equal(getAdjacentIndex(state, 'previous'), 0);
  assert.equal(getAdjacentIndex(state, 'next'), 2);
  selectQueueIndex(state, 2);
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
  selectQueueIndex(state, next);
  assert.equal(canGoPrevious(state), true);
  assert.equal(getAdjacentIndex(state, 'previous'), 1);
});

test('single item queues disable adjacent navigation', () => {
  const state = createPlayerState();
  replaceQueue(state, { bvid: 'BV1111111111', title: '单集', duration: 10 });
  assert.equal(canGoPrevious(state), false);
  assert.equal(canGoNext(state), false);
  assert.equal(getAdjacentIndex(state, 'next'), null);
});
