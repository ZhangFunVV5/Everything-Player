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
import { createApplicationServices } from './composition-root.js';
import { runVersionedResolve } from './services/versioned-resolve.js';
import type { FavoriteFolder } from './types/favorites.js';
import type { BvInput, ResolvedVideo } from './types/media.js';
import type {
  PlayMode,
  PlayerPage,
  QueueDirection,
} from './types/player.js';

interface LoadBvidOptions {
  replace?: boolean;
  targetIndex?: number | null;
  closeList?: boolean;
  favoriteFolder?: FavoriteFolder | null;
}

interface ModePresentation {
  icon: string;
  label: string;
  aria: string;
}

function requiredElement<T extends Element>(element: T | null, message: string): T {
  if (!element) throw new Error(message);
  return element;
}

const state = createPlayerState();
let currentVideo: ResolvedVideo | null = null;
let loading = false;
let requestVersion = 0;
let seeking = false;
let toastTimer: ReturnType<typeof setTimeout> | null = null;
const applicationServices = createApplicationServices();

const audio = requiredElement(
  document.querySelector<HTMLAudioElement>('#audio-player'),
  '播放器元素不存在',
);
const playerPage = requiredElement(
  document.querySelector<HTMLElement>('#player-page'),
  '播放器页面元素不存在',
);
const profilePage = requiredElement(
  document.querySelector<HTMLElement>('#profile-page'),
  '个人页面元素不存在',
);
const emptyState = requiredElement(
  document.querySelector<HTMLElement>('#empty-state'),
  '空状态元素不存在',
);
const playerContent = requiredElement(
  document.querySelector<HTMLElement>('#player-content'),
  '播放器内容元素不存在',
);
const addButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#add-button'),
  '添加按钮元素不存在',
);
const coverImage = requiredElement(
  document.querySelector<HTMLImageElement>('#cover-image'),
  '封面元素不存在',
);
const coverPlaceholder = requiredElement(
  document.querySelector<HTMLElement>('#cover-placeholder'),
  '封面占位元素不存在',
);
const loadingMask = requiredElement(
  document.querySelector<HTMLElement>('#loading-mask'),
  '加载元素不存在',
);
const videoTitle = requiredElement(
  document.querySelector<HTMLElement>('#video-title'),
  '标题元素不存在',
);
const videoOwner = requiredElement(
  document.querySelector<HTMLElement>('#video-owner'),
  '作者元素不存在',
);
const progress = requiredElement(
  document.querySelector<HTMLInputElement>('#progress'),
  '进度元素不存在',
);
const currentTime = requiredElement(
  document.querySelector<HTMLElement>('#current-time'),
  '当前时间元素不存在',
);
const durationTime = requiredElement(
  document.querySelector<HTMLElement>('#duration-time'),
  '总时长元素不存在',
);
const modeButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#mode-button'),
  '模式按钮元素不存在',
);
const modeIcon = requiredElement(
  document.querySelector<HTMLElement>('#mode-icon'),
  '模式图标元素不存在',
);
const modeLabel = requiredElement(
  document.querySelector<HTMLElement>('#mode-label'),
  '模式标签元素不存在',
);
const previousButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#previous-button'),
  '上一首按钮元素不存在',
);
const playButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#play-button'),
  '播放按钮元素不存在',
);
const playIcon = requiredElement(
  document.querySelector<SVGSVGElement>('#play-icon'),
  '播放图标元素不存在',
);
const pauseIcon = requiredElement(
  document.querySelector<SVGSVGElement>('#pause-icon'),
  '暂停图标元素不存在',
);
const nextButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#next-button'),
  '下一首按钮元素不存在',
);
const queueButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#queue-button'),
  '播放列表按钮元素不存在',
);
const favoriteButton = requiredElement(
  document.querySelector<HTMLButtonElement>('#favorite-button'),
  '收藏按钮元素不存在',
);
const toast = requiredElement(
  document.querySelector<HTMLElement>('#toast'),
  '提示元素不存在',
);

const bvDialog = requiredElement(
  document.querySelector<HTMLDialogElement>('#bv-dialog'),
  'BV 对话框元素不存在',
);
const bvForm = requiredElement(
  document.querySelector<HTMLFormElement>('#bv-form'),
  'BV 表单元素不存在',
);
const bvInput = requiredElement(
  document.querySelector<HTMLInputElement>('#bv-input'),
  'BV 输入元素不存在',
);
const bvError = requiredElement(
  document.querySelector<HTMLElement>('#bv-error'),
  'BV 错误元素不存在',
);
const bvSubmit = requiredElement(
  document.querySelector<HTMLButtonElement>('#bv-submit'),
  'BV 提交按钮元素不存在',
);
const bvCancel = requiredElement(
  document.querySelector<HTMLButtonElement>('#bv-cancel'),
  'BV 取消按钮元素不存在',
);
const bvClose = requiredElement(
  document.querySelector<HTMLButtonElement>('#bv-close'),
  'BV 关闭按钮元素不存在',
);

const queueDialog = requiredElement(
  document.querySelector<HTMLDialogElement>('#queue-dialog'),
  '播放列表对话框元素不存在',
);
const queueTitle = requiredElement(
  document.querySelector<HTMLElement>('#queue-title'),
  '播放列表标题元素不存在',
);
const queueCount = requiredElement(
  document.querySelector<HTMLElement>('#queue-count'),
  '播放列表数量元素不存在',
);
const queueList = requiredElement(
  document.querySelector<HTMLElement>('#queue-list'),
  '播放列表元素不存在',
);
const queueAdd = requiredElement(
  document.querySelector<HTMLButtonElement>('#queue-add'),
  '播放列表添加按钮元素不存在',
);
const queueClose = requiredElement(
  document.querySelector<HTMLButtonElement>('#queue-close'),
  '播放列表关闭按钮元素不存在',
);
const favorites = initializeFavorites({
  getVideo: () => currentVideo,
  isLoading: () => loading,
  notify: showToast,
  favoritesApplication: applicationServices.favorites,
  legacyMigration: applicationServices.legacyMigration,
  play: async (folder, bvid) => {
    if (loading) return false;
    const success = await loadBvid(bvid, { favoriteFolder: folder });
    if (success) showPage('player');
    return success;
  },
});

const modePresentation: Record<PlayMode, ModePresentation> = {
  sequence: { icon: '↦', label: '顺序', aria: '当前为顺序播放' },
  shuffle: { icon: '⤨', label: '随机', aria: '当前为随机播放' },
  repeat: { icon: '↻', label: '循环', aria: '当前为列表循环' },
};

function formatTime(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return '0:00';
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const remaining = whole % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remaining).padStart(2, '0')}`
    : `${minutes}:${String(remaining).padStart(2, '0')}`;
}

function showToast(message: string, duration = 2600): void {
  if (toastTimer !== null) clearTimeout(toastTimer);
  toast.textContent = message;
  toast.hidden = false;
  toastTimer = setTimeout(() => {
    toast.hidden = true;
  }, duration);
}

function setLoading(value: boolean): void {
  loading = value;
  bvInput.disabled = value;
  bvSubmit.disabled = value;
  bvSubmit.textContent = value ? '解析中…' : '确定';
  loadingMask.hidden = !value || !currentVideo;
  updateControls();
  renderQueue();
}

function showPage(page: PlayerPage): void {
  state.page = page;
  const showPlayer = page === 'player';
  playerPage.hidden = !showPlayer;
  profilePage.hidden = showPlayer;
  for (const item of document.querySelectorAll<HTMLButtonElement>('.nav-item')) {
    const active = item.dataset.page === page;
    item.classList.toggle('active', active);
    if (active) item.setAttribute('aria-current', 'page');
    else item.removeAttribute('aria-current');
  }
}

function openBvDialog(): void {
  bvError.textContent = '';
  if (!bvDialog.open) bvDialog.showModal();
  requestAnimationFrame(() => {
    bvInput.focus();
    bvInput.select();
  });
}

function closeBvDialog(): void {
  if (!loading && bvDialog.open) bvDialog.close();
}

function updateCover(url: string): void {
  coverImage.classList.remove('visible');
  coverImage.hidden = true;
  coverPlaceholder.hidden = false;
  coverImage.removeAttribute('src');
  if (!url) return;
  coverImage.src = url;
  coverImage.hidden = false;
}

function updatePlayButton(): void {
  const playing = !audio.paused && !audio.ended;
  playIcon.toggleAttribute('hidden', playing);
  pauseIcon.toggleAttribute('hidden', !playing);
  playButton.setAttribute('aria-label', playing ? '暂停' : '播放');
}

function updateProgress(): void {
  const duration = Number.isFinite(audio.duration) ? audio.duration : Number(currentVideo?.duration || 0);
  const position = Number.isFinite(audio.currentTime) ? audio.currentTime : 0;
  if (!seeking) {
    progress.value = duration > 0 ? String(Math.round((position / duration) * 1000)) : '0';
    currentTime.textContent = formatTime(position);
  }
  durationTime.textContent = formatTime(duration);
  progress.disabled = loading || !Number.isFinite(audio.duration) || audio.duration <= 0;
  progress.style.setProperty('--progress', `${Number(progress.value) / 10}%`);
}

function updateControls(): void {
  favoriteButton.disabled = loading || !currentVideo;
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

function renderCurrentVideo(): void {
  const hasVideo = Boolean(currentVideo);
  emptyState.hidden = hasVideo;
  playerContent.hidden = !hasVideo;
  addButton.hidden = hasVideo || state.page !== 'player';
  if (!currentVideo) return updateControls();

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

function renderQueue(): void {
  queueTitle.textContent = state.queueTitle;
  queueCount.textContent = `${state.queue.length} 个视频`;
  queueList.replaceChildren();
  let lastSection: string | null = null;

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

function openQueue(): void {
  renderQueue();
  if (!queueDialog.open) queueDialog.showModal();
  requestAnimationFrame(() => {
    queueList.querySelector('.queue-item.current')?.scrollIntoView({ block: 'center' });
  });
}

function closeQueue(): void {
  if (queueDialog.open) queueDialog.close();
}

async function loadBvid(
  input: BvInput,
  {
    replace = true,
    targetIndex = null,
    closeList = false,
    favoriteFolder = null,
  }: LoadBvidOptions = {},
): Promise<boolean> {
  const version = ++requestVersion;
  return runVersionedResolve({
    resolve: () => applicationServices.player.resolveForPlayback(input),
    isCurrent: () => version === requestVersion,
    snapshot: () => ({
      currentIndex: state.currentIndex,
      randomHistory: [...state.randomHistory],
      randomCursor: state.randomCursor,
    }),
    start: () => {
      applicationServices.player.invalidatePendingAudioLoad();
      setLoading(true);
      bvError.textContent = '';
    },
    succeed: async (resolved) => {
      await applicationServices.player.loadAudio(audio, resolved, (message) => {
        if (version !== requestVersion) return;
        showToast(message, 4000);
        updatePlayButton();
      });
      if (version !== requestVersion) throw new Error('已被新请求取代');
      if (favoriteFolder) {
        replaceFavoriteQueue(state, favoriteFolder, resolved.bvid);
      } else if (replace) {
        replaceQueue(state, resolved);
      } else if (!updateCurrentQueueItem(state, resolved)) {
        replaceQueue(state, resolved);
      } else if (targetIndex !== null) {
        selectQueueIndex(state, targetIndex);
      }

      currentVideo = resolved;
      applicationServices.player.commitPlayback();
      favorites.refreshStar();
      setLoading(false);
      renderCurrentVideo();
      renderQueue();
      if (bvDialog.open) bvDialog.close();
      if (closeList) closeQueue();

      try {
        await audio.play();
      } catch {
        if (version === requestVersion) showToast('已加载，请点击播放按钮开始');
      }
    },
    restore: (snapshot) => {
      state.currentIndex = snapshot.currentIndex;
      state.randomHistory = snapshot.randomHistory;
      state.randomCursor = snapshot.randomCursor;
    },
    fail: async (message) => {
      try {
        await applicationServices.player.restoreAfterFailedResolve(audio, (error) => {
          if (version === requestVersion) showToast(error, 4000);
        }, () => version === requestVersion);
      } catch (error) {
        if (version === requestVersion) showToast(error instanceof Error ? error.message : String(error), 4000);
      }
      if (version !== requestVersion) return;
      if (bvDialog.open) bvError.textContent = message;
      else showToast(message, 3800);
      setLoading(false);
    },
  });
}

async function loadQueueIndex(
  index: number,
  { closeList = false }: { closeList?: boolean } = {},
): Promise<boolean> {
  if (loading || index < 0 || index >= state.queue.length) return false;
  if (index === state.currentIndex) {
    if (closeList) closeQueue();
    return true;
  }
  return loadBvid(state.queue[index].bvid, { replace: false, targetIndex: index, closeList });
}

async function navigate(direction: QueueDirection, automatic = false): Promise<void> {
  if (loading) return;

  if (state.queue.length === 1) {
    if (automatic && state.mode === 'repeat') {
      audio.currentTime = 0;
      if (currentVideo) {
        try {
          await applicationServices.player.loadAudio(audio, currentVideo, (message) => showToast(message, 4000));
          applicationServices.player.commitPlayback();
        } catch (error) {
          showToast(error instanceof Error ? error.message : String(error), 4000);
          return;
        }
      }
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

for (const item of document.querySelectorAll<HTMLButtonElement>('.nav-item')) {
  item.addEventListener('click', (): void => {
    const page = item.dataset.page;
    if (page === 'player' || page === 'profile') showPage(page);
  });
}

addButton.addEventListener('click', (): void => openBvDialog());
bvCancel.addEventListener('click', (): void => closeBvDialog());
bvClose.addEventListener('click', (): void => closeBvDialog());
bvDialog.addEventListener('click', (event: MouseEvent): void => {
  if (event.target === bvDialog) closeBvDialog();
});
bvForm.addEventListener('submit', async (event: SubmitEvent): Promise<void> => {
  event.preventDefault();
  await loadBvid(bvInput.value, { replace: true });
});

modeButton.addEventListener('click', (): void => {
  const mode = cycleMode(state);
  updateControls();
  const name = modePresentation[mode].label;
  showToast(`已切换为${name}播放`);
});

previousButton.addEventListener('click', (): void => { void navigate('previous'); });
nextButton.addEventListener('click', (): void => { void navigate('next'); });
playButton.addEventListener('click', async (): Promise<void> => {
  if (!currentVideo || loading) return;
  if (!audio.paused) {
    audio.pause();
    return;
  }
  try {
    if (audio.ended) {
      await applicationServices.player.loadAudio(audio, currentVideo, (message) => showToast(message, 4000));
      applicationServices.player.commitPlayback();
    } else {
      await applicationServices.player.resumeAudio();
    }
    await audio.play();
  } catch {
    showToast('播放失败，请重新添加该 BV 号');
  }
});

queueButton.addEventListener('click', (): void => openQueue());
queueClose.addEventListener('click', (): void => closeQueue());
queueDialog.addEventListener('click', (event: MouseEvent): void => {
  if (event.target === queueDialog) closeQueue();
});
queueAdd.addEventListener('click', (): void => {
  closeQueue();
  bvInput.value = '';
  openBvDialog();
});
queueList.addEventListener('click', async (event: MouseEvent): Promise<void> => {
  if (!(event.target instanceof Element)) return;
  const item = event.target.closest<HTMLButtonElement>('.queue-item[data-index]');
  if (!item || item.disabled) return;
  const indexValue = item.dataset.index;
  if (indexValue === undefined) return;
  const index = Number(indexValue);
  if (!Number.isInteger(index)) return;
  await loadQueueIndex(index, { closeList: true });
});

progress.addEventListener('input', (): void => {
  const duration = audio.duration;
  if (!Number.isFinite(duration) || duration <= 0) return;
  seeking = true;
  const preview = (Number(progress.value) / 1000) * duration;
  currentTime.textContent = formatTime(preview);
  progress.style.setProperty('--progress', `${Number(progress.value) / 10}%`);
});
progress.addEventListener('change', (): void => {
  const duration = audio.duration;
  if (Number.isFinite(duration) && duration > 0) {
    const seconds = (Number(progress.value) / 1000) * duration;
    audio.currentTime = seconds;
    void applicationServices.player.seekAudio(seconds).catch((error: unknown) => {
      showToast(error instanceof Error ? error.message : String(error), 4000);
    });
  }
  seeking = false;
  updateProgress();
});

coverImage.addEventListener('load', (): void => {
  coverImage.classList.add('visible');
  coverPlaceholder.hidden = true;
});
coverImage.addEventListener('error', (): void => {
  coverImage.hidden = true;
  coverPlaceholder.hidden = false;
});

audio.addEventListener('play', (): void => updatePlayButton());
audio.addEventListener('pause', (): void => updatePlayButton());
audio.addEventListener('loadedmetadata', (): void => updateProgress());
audio.addEventListener('durationchange', (): void => updateProgress());
audio.addEventListener('timeupdate', (): void => updateProgress());
audio.addEventListener('ended', (): void => { void navigate('next', true); });
window.addEventListener('pagehide', (): void => { void applicationServices.player.disposeAudio(); });
audio.addEventListener('error', (): void => {
  if (audio.currentSrc && currentVideo && !loading) {
    showToast('音频播放失败或地址已失效，请重新添加 BV 号', 4000);
  }
  updatePlayButton();
});

document.addEventListener('keydown', (event: KeyboardEvent): void => {
  if (event.key !== 'Escape') return;
  if (queueDialog.open) closeQueue();
  else if (bvDialog.open) closeBvDialog();
});

showPage('player');
renderCurrentVideo();
renderQueue();

if (import.meta.env.VITE_P7_OVERLAP_PROOF === '1') {
  void import('./p7-overlap-proof.js');
}
