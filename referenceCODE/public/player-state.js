export const PLAY_MODES = Object.freeze(['sequence', 'shuffle', 'repeat']);

export function createPlayerState() {
  return {
    page: 'player',
    sourceType: 'single',
    sourceId: null,
    mode: 'sequence',
    queueTitle: '播放列表',
    queue: [],
    currentIndex: -1,
    randomHistory: [],
    randomCursor: -1,
  };
}

function normalizeEpisode(episode, sectionTitle = '') {
  return {
    bvid: episode.bvid,
    title: episode.title || episode.bvid,
    duration: Number.isFinite(episode.duration) ? episode.duration : null,
    coverUrl: episode.coverUrl || '',
    sectionTitle,
  };
}

export function buildQueue(resolved) {
  const byBvid = new Map();
  for (const section of resolved.collection?.sections || []) {
    for (const episode of section.episodes || []) {
      if (!episode.bvid) continue;
      const key = episode.bvid.toLowerCase();
      if (!byBvid.has(key)) byBvid.set(key, normalizeEpisode(episode, section.title || ''));
    }
  }

  const current = normalizeEpisode(resolved);
  const currentKey = resolved.bvid.toLowerCase();
  if (byBvid.has(currentKey)) {
    byBvid.set(currentKey, { ...byBvid.get(currentKey), ...current });
  } else {
    byBvid.set(currentKey, current);
  }

  return {
    title: resolved.collection?.title || resolved.title || '播放列表',
    items: [...byBvid.values()],
    currentIndex: [...byBvid.keys()].indexOf(currentKey),
  };
}

export function replaceQueue(state, resolved) {
  state.sourceType = resolved.collection ? 'collection' : 'single';
  state.sourceId = resolved.collection?.id ?? resolved.bvid;
  const queue = buildQueue(resolved);
  state.queueTitle = queue.title;
  state.queue = queue.items;
  state.currentIndex = queue.currentIndex;
  resetRandomHistory(state);
}

export function replaceFavoriteQueue(state, folder, bvid) {
  state.sourceType = 'favorite';
  state.sourceId = folder.id;
  state.queueTitle = folder.name;
  state.queue = folder.items.map(item => ({ ...item, sectionTitle: '' }));
  state.currentIndex = state.queue.findIndex(item => item.bvid === bvid);
  resetRandomHistory(state);
}

export function selectQueueIndex(state, index) {
  if (index < 0 || index >= state.queue.length) return false;
  state.currentIndex = index;
  if (state.mode === 'shuffle') {
    const activeHistoryIndex = state.randomHistory[state.randomCursor];
    if (activeHistoryIndex !== index) {
      state.randomHistory = state.randomHistory.slice(0, state.randomCursor + 1);
      state.randomHistory.push(index);
      state.randomCursor = state.randomHistory.length - 1;
    }
  }
  return true;
}

export function updateCurrentQueueItem(state, resolved) {
  const index = state.queue.findIndex(
    (item) => item.bvid === resolved.bvid,
  );
  if (index < 0) return false;
  state.queue[index] = {
    ...state.queue[index],
    title: resolved.title || state.queue[index].title,
    duration: resolved.duration ?? state.queue[index].duration,
    coverUrl: resolved.coverUrl || state.queue[index].coverUrl,
  };
  return selectQueueIndex(state, index);
}

export function cycleMode(state) {
  const index = PLAY_MODES.indexOf(state.mode);
  state.mode = PLAY_MODES[(index + 1) % PLAY_MODES.length];
  resetRandomHistory(state);
  return state.mode;
}

export function resetRandomHistory(state) {
  if (state.currentIndex >= 0) {
    state.randomHistory = [state.currentIndex];
    state.randomCursor = 0;
  } else {
    state.randomHistory = [];
    state.randomCursor = -1;
  }
}

function randomOtherIndex(length, currentIndex, random) {
  const value = Math.min(0.999999999, Math.max(0, random()));
  const candidate = Math.floor(value * (length - 1));
  return candidate >= currentIndex ? candidate + 1 : candidate;
}

export function getAdjacentIndex(state, direction, random = Math.random) {
  const length = state.queue.length;
  if (length <= 1 || state.currentIndex < 0) return null;

  if (state.mode === 'repeat') {
    return direction === 'next'
      ? (state.currentIndex + 1) % length
      : (state.currentIndex - 1 + length) % length;
  }

  if (state.mode === 'sequence') {
    const candidate = state.currentIndex + (direction === 'next' ? 1 : -1);
    return candidate >= 0 && candidate < length ? candidate : null;
  }

  if (direction === 'previous') {
    if (state.randomCursor <= 0) return null;
    state.randomCursor -= 1;
    return state.randomHistory[state.randomCursor];
  }

  if (state.randomCursor < state.randomHistory.length - 1) {
    state.randomCursor += 1;
    return state.randomHistory[state.randomCursor];
  }

  const nextIndex = randomOtherIndex(length, state.currentIndex, random);
  state.randomHistory.push(nextIndex);
  state.randomCursor = state.randomHistory.length - 1;
  return nextIndex;
}

export function canGoPrevious(state) {
  if (state.queue.length <= 1) return false;
  if (state.mode === 'repeat') return true;
  if (state.mode === 'shuffle') return state.randomCursor > 0;
  return state.currentIndex > 0;
}

export function canGoNext(state) {
  if (state.queue.length <= 1) return false;
  if (state.mode === 'repeat' || state.mode === 'shuffle') return true;
  return state.currentIndex < state.queue.length - 1;
}
