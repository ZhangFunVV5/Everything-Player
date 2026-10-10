import {
  replaceFavoriteQueue,
  canGoNext,
  canGoPrevious,
  createPlayerState,
  cycleMode,
  getAdjacentIndex,
  replaceQueue,
  selectQueueIndex,
  updateCurrentQueueItem,
} from './player-state.js';
import { initializeFavorites } from './favorites.js';

const state = createPlayerState();
let currentVideo = null;
let loading = false;
let requestVersion = 0;
let seeking = false;
let toastTimer = null;

const audio = document.querySelector('#audio-player');
const playerPage = document.querySelector('#player-page');
const profilePage = document.querySelector('#profile-page');
const emptyState = document.querySelector('#empty-state');
const playerContent = document.querySelector('#player-content');
const addButton = document.querySelector('#add-button');
const coverImage = document.querySelector('#cover-image');
const coverPlaceholder = document.querySelector('#cover-placeholder');
const loadingMask = document.querySelector('#loading-mask');
const videoTitle = document.querySelector('#video-title');
const videoOwner = document.querySelector('#video-owner');
const progress = document.querySelector('#progress');
const currentTime = document.querySelector('#current-time');
const durationTime = document.querySelector('#duration-time');
const modeButton = document.querySelector('#mode-button');
const modeIcon = document.querySelector('#mode-icon');
const modeLabel = document.querySelector('#mode-label');
const previousButton = document.querySelector('#previous-button');
const playButton = document.querySelector('#play-button');
const playIcon = document.querySelector('#play-icon');
const pauseIcon = document.querySelector('#pause-icon');
const nextButton = document.querySelector('#next-button');
const queueButton = document.querySelector('#queue-button');
const toast = document.querySelector('#toast');

const bvDialog = document.querySelector('#bv-dialog');
const bvForm = document.querySelector('#bv-form');
const bvInput = document.querySelector('#bv-input');
const bvError = document.querySelector('#bv-error');
const bvSubmit = document.querySelector('#bv-submit');
const bvCancel = document.querySelector('#bv-cancel');
const bvClose = document.querySelector('#bv-close');

const queueDialog = document.querySelector('#queue-dialog');
const queueTitle = document.querySelector('#queue-title');
const queueCount = document.querySelector('#queue-count');
const queueList = document.querySelector('#queue-list');
const queueAdd = document.querySelector('#queue-add');
const queueClose = document.querySelector('#queue-close');
const favorites = initializeFavorites({
  getVideo: () => currentVideo,
  isLoading: () => loading,
  notify: showToast,
  play: async (folder, bvid) => {
    if (loading) return false;
    const success = await loadBvid(bvid, { favoriteFolder: folder });
    if (success) showPage('player');
    return success;
  },
});

const modePresentation = {
  sequence: { icon: '↦', label: '顺序', aria: '当前为顺序播放' },
  shuffle: { icon: '⤨', label: '随机', aria: '当前为随机播放' },
  repeat: { icon: '↻', label: '循环', aria: '当前为列表循环' },
};

function formatTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '0:00';
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const remaining = whole % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remaining).padStart(2, '0')}`
    : `${minutes}:${String(remaining).padStart(2, '0')}`;
}

function showToast(message, duration = 2600) {
  clearTimeout(toastTimer);
  toast.textContent = message;
  toast.hidden = false;
  toastTimer = setTimeout(() => {
    toast.hidden = true;
  }, duration);
}

function setLoading(value) {
  loading = value;
  bvInput.disabled = value;
  bvSubmit.disabled = value;
  bvSubmit.textContent = value ? '解析中…' : '确定';
  loadingMask.hidden = !value || !currentVideo;
  updateControls();
  renderQueue();
}

function showPage(page) {
  state.page = page;
  const showPlayer = page === 'player';
  playerPage.hidden = !showPlayer;
  profilePage.hidden = showPlayer;
  for (const item of document.querySelectorAll('.nav-item')) {
    const active = item.dataset.page === page;
    item.classList.toggle('active', active);
    if (active) item.setAttribute('aria-current', 'page');
    else item.removeAttribute('aria-current');
  }
}

function openBvDialog() {
  bvError.textContent = '';
  if (!bvDialog.open) bvDialog.showModal();
  requestAnimationFrame(() => {
    bvInput.focus();
    bvInput.select();
  });
}

function closeBvDialog() {
  if (!loading && bvDialog.open) bvDialog.close();
}

function updateCover(url) {
  coverImage.classList.remove('visible');
  coverImage.hidden = true;
  coverPlaceholder.hidden = false;
  coverImage.removeAttribute('src');
  if (!url) return;
  coverImage.src = url;
  coverImage.hidden = false;
}

function updatePlayButton() {
  const playing = !audio.paused && !audio.ended;
  playIcon.toggleAttribute('hidden', playing);
  pauseIcon.toggleAttribute('hidden', !playing);
  playButton.setAttribute('aria-label', playing ? '暂停' : '播放');
}

function updateProgress() {
  const duration = Number.isFinite(audio.duration) ? audio.duration : Number(currentVideo?.duration || 0);
  const position = Number.isFinite(audio.currentTime) ? audio.currentTime : 0;
  if (!seeking) {
    progress.value = duration > 0 ? String(Math.round((position / duration) * 1000)) : '0';
    currentTime.textContent = formatTime(position);
  }
  durationTime.textContent = formatTime(duration);
  progress.disabled = loading || !Number.isFinite(audio.duration) || audio.duration <= 0;
  progress.style.setProperty('--progress', `${progress.value / 10}%`);
}

function updateControls() {
  document.querySelector('#favorite-button').disabled = loading || !currentVideo;
  const presentation = modePresentation[state.mode];
  modeIcon.textContent = presentation.icon;
  modeLabel.textContent = presentation.label;
  modeButton.setAttribute('aria-label', presentation.aria);

  const hasVideo = Boolean(currentVideo);
  playButton.disabled = loading || !hasVideo;
  modeButton.disabled = loading || !hasVideo;
  previousButton.disabled = loading || !canGoPrevious(state);
  nextButton.disabled = loading || !canGoNext(state);
  queueButton.disabled = loading || !hasVideo;
  updatePlayButton();
  updateProgress();
}

function renderCurrentVideo() {
  const hasVideo = Boolean(currentVideo);
  emptyState.hidden = hasVideo;
  playerContent.hidden = !hasVideo;
  addButton.hidden = hasVideo || state.page !== 'player';
  if (!hasVideo) return updateControls();

  videoTitle.textContent = currentVideo.title;
  const distinctPageTitle =
    currentVideo.pageTitle && currentVideo.pageTitle !== currentVideo.title
      ? currentVideo.pageTitle
      : '';
  videoOwner.textContent = [currentVideo.owner && `UP 主：${currentVideo.owner}`, distinctPageTitle]
    .filter(Boolean)
    .join(' · ');
  updateCover(currentVideo.coverUrl);
  updateControls();
}

function renderQueue() {
  queueTitle.textContent = state.queueTitle;
  queueCount.textContent = `${state.queue.length} 个视频`;
  queueList.replaceChildren();
  let lastSection = null;

  state.queue.forEach((item, index) => {
    if (item.sectionTitle && item.sectionTitle !== lastSection) {
      const sectionTitle = document.createElement('p');
      sectionTitle.className = 'queue-section-title';
      sectionTitle.textContent = item.sectionTitle;
      queueList.append(sectionTitle);
      lastSection = item.sectionTitle;
    }

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'queue-item';
    button.dataset.index = String(index);
    button.disabled = loading;
    if (index === state.currentIndex) {
      button.classList.add('current');
      button.setAttribute('aria-current', 'true');
    }

    const number = document.createElement('span');
    number.className = 'queue-index';
    number.textContent = String(index + 1).padStart(2, '0');
    const title = document.createElement('span');
    title.className = 'queue-item-title';
    title.textContent = item.title;
    const duration = document.createElement('time');
    duration.className = 'queue-duration';
    duration.textContent = formatTime(item.duration);
    button.append(number, title, duration);
    queueList.append(button);
  });
}

function openQueue() {
  renderQueue();
  if (!queueDialog.open) queueDialog.showModal();
  requestAnimationFrame(() => {
    queueList.querySelector('.queue-item.current')?.scrollIntoView({ block: 'center' });
  });
}

function closeQueue() {
  if (queueDialog.open) queueDialog.close();
}

async function loadBvid(input, { replace = true, targetIndex = null, closeList = false, favoriteFolder = null } = {}) {
  const version = ++requestVersion;
  const oldIndex = state.currentIndex;
  const oldHistory = [...state.randomHistory];
  const oldCursor = state.randomCursor;
  setLoading(true);
  bvError.textContent = '';

  try {
    const response = await fetch('/api/resolve', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ input }),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || '解析失败');
    if (version !== requestVersion) return false;

    if (favoriteFolder) {
      replaceFavoriteQueue(state, favoriteFolder, result.bvid);
    } else if (replace) {
      replaceQueue(state, result);
    } else if (!updateCurrentQueueItem(state, result)) {
      replaceQueue(state, result);
    } else if (targetIndex !== null) {
      selectQueueIndex(state, targetIndex);
    }

    currentVideo = result;
    favorites.refreshStar();
    audio.pause();
    audio.src = result.audioUrl;
    audio.load();
    setLoading(false);
    renderCurrentVideo();
    renderQueue();
    if (bvDialog.open) bvDialog.close();
    if (closeList) closeQueue();

    try {
      await audio.play();
    } catch {
      showToast('已加载，请点击播放按钮开始');
    }
    return true;
  } catch (error) {
    if (version !== requestVersion) return false;
    state.currentIndex = oldIndex;
    state.randomHistory = oldHistory;
    state.randomCursor = oldCursor;
    const message = error instanceof Error ? error.message : '解析失败';
    if (bvDialog.open) bvError.textContent = message;
    else showToast(message, 3800);
    setLoading(false);
    return false;
  }
}

async function loadQueueIndex(index, { closeList = false } = {}) {
  if (loading || index < 0 || index >= state.queue.length) return false;
  if (index === state.currentIndex) {
    if (closeList) closeQueue();
    return true;
  }
  return loadBvid(state.queue[index].bvid, { replace: false, targetIndex: index, closeList });
}

async function navigate(direction, automatic = false) {
  if (loading) return;

  if (state.queue.length === 1) {
    if (automatic && state.mode === 'repeat') {
      audio.currentTime = 0;
      await audio.play().catch(() => showToast('请点击播放按钮继续'));
    }
    return;
  }

  const oldHistory = [...state.randomHistory];
  const oldCursor = state.randomCursor;
  const index = getAdjacentIndex(state, direction);
  if (index === null) {
    if (automatic) updatePlayButton();
    return;
  }
  const success = await loadQueueIndex(index);
  if (!success) {
    state.randomHistory = oldHistory;
    state.randomCursor = oldCursor;
  }
}

for (const item of document.querySelectorAll('.nav-item')) {
  item.addEventListener('click', () => showPage(item.dataset.page));
}

addButton.addEventListener('click', openBvDialog);
bvCancel.addEventListener('click', closeBvDialog);
bvClose.addEventListener('click', closeBvDialog);
bvDialog.addEventListener('click', (event) => {
  if (event.target === bvDialog) closeBvDialog();
});
bvForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  await loadBvid(bvInput.value, { replace: true });
});

modeButton.addEventListener('click', () => {
  const mode = cycleMode(state);
  updateControls();
  const name = modePresentation[mode].label;
  showToast(`已切换为${name}播放`);
});

previousButton.addEventListener('click', () => navigate('previous'));
nextButton.addEventListener('click', () => navigate('next'));
playButton.addEventListener('click', async () => {
  if (!currentVideo || loading) return;
  if (!audio.paused) return audio.pause();
  if (audio.ended) audio.currentTime = 0;
  try {
    await audio.play();
  } catch {
    showToast('播放失败，请重新添加该 BV 号');
  }
});

queueButton.addEventListener('click', openQueue);
queueClose.addEventListener('click', closeQueue);
queueDialog.addEventListener('click', (event) => {
  if (event.target === queueDialog) closeQueue();
});
queueAdd.addEventListener('click', () => {
  closeQueue();
  bvInput.value = '';
  openBvDialog();
});
queueList.addEventListener('click', async (event) => {
  const item = event.target.closest('.queue-item[data-index]');
  if (!item || item.disabled) return;
  await loadQueueIndex(Number(item.dataset.index), { closeList: true });
});

progress.addEventListener('input', () => {
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return;
  seeking = true;
  const preview = (Number(progress.value) / 1000) * duration;
  currentTime.textContent = formatTime(preview);
  progress.style.setProperty('--progress', `${progress.value / 10}%`);
});
progress.addEventListener('change', () => {
  const duration = audio.duration;
  if (Number.isFinite(duration) && duration > 0) {
    audio.currentTime = (Number(progress.value) / 1000) * duration;
  }
  seeking = false;
  updateProgress();
});

coverImage.addEventListener('load', () => {
  coverImage.classList.add('visible');
  coverPlaceholder.hidden = true;
});
coverImage.addEventListener('error', () => {
  coverImage.hidden = true;
  coverPlaceholder.hidden = false;
});

audio.addEventListener('play', updatePlayButton);
audio.addEventListener('pause', updatePlayButton);
audio.addEventListener('loadedmetadata', updateProgress);
audio.addEventListener('durationchange', updateProgress);
audio.addEventListener('timeupdate', updateProgress);
audio.addEventListener('ended', () => navigate('next', true));
audio.addEventListener('error', () => {
  if (audio.currentSrc && currentVideo && !loading) {
    showToast('音频播放失败或地址已失效，请重新添加 BV 号', 4000);
  }
  updatePlayButton();
});

document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  if (queueDialog.open) closeQueue();
  else if (bvDialog.open) closeBvDialog();
});

showPage('player');
renderCurrentVideo();
renderQueue();
