import type {
  Bvid,
  ResolvedVideo,
} from './types/media.js';
import type { ServiceResult } from './types/errors.js';
import type {
  FavoriteAddRequest,
  FavoriteCreateRequest,
  FavoriteFolder,
} from './types/favorites.js';
import type { FavoritesApplicationService } from './services/favorites-application-service.js';
import type { LegacyMigrationApplicationService } from './services/legacy-migration-application-service.js';
import type { LegacyMigrationResult } from './types/legacy-migration.js';

type Screen = 'home' | 'list' | 'detail';
type ButtonAction = () => void | Promise<void>;
type FormBuilder<TFields> = (form: HTMLFormElement) => TFields;
type FormSubmit<TFields> = (fields: TFields) => void | Promise<void>;

interface InitializeFavoritesOptions {
  getVideo: () => ResolvedVideo | null;
  isLoading: () => boolean;
  notify: (message: string, duration?: number) => void;
  play: (folder: FavoriteFolder, bvid: Bvid) => Promise<boolean>;
  favoritesApplication: FavoritesApplicationService;
  legacyMigration: LegacyMigrationApplicationService | null;
}

interface FavoritesController {
  refreshStar: (reload?: boolean) => Promise<void>;
}

export function migrationNotification(result: LegacyMigrationResult): string {
  const details = result.files
    .filter((file) => file.reason)
    .map((file) => `${file.name}：${file.reason}`)
    .join('\n');
  if (result.recordError) {
    const recordFailure = `迁移记录未能保存：${result.recordError}`;
    const retentionNotice = '已复制的收藏夹已保留，不会自动删除。';
    return [recordFailure, retentionNotice, details].filter(Boolean).join('\n');
  }
  return details || `已迁移 ${result.importedFolders} 个收藏夹`;
}

function errorMessage(error: unknown, fallback = '收藏夹操作失败'): string {
  return error instanceof Error ? error.message : fallback;
}

function requiredElement<T extends Element>(element: T | null, message: string): T {
  if (!element) throw new Error(message);
  return element;
}

function unwrap<T>(result: ServiceResult<T>): T {
  if (!result.ok) throw new Error(result.error.message);
  return result.value;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  text?: string,
  className?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (text) node.textContent = text;
  if (className) node.className = className;
  return node;
}

function button(
  text: string,
  action: ButtonAction,
  className = 'secondary-button',
): HTMLButtonElement {
  const node = el('button', text, className);
  node.type = 'button';
  node.onclick = action;
  return node;
}

function cover(url: string): HTMLSpanElement {
  const box = el('span', '♪', 'favorite-cover');
  if (url) {
    const img = el('img');
    img.src = url;
    img.referrerPolicy = 'no-referrer';
    img.alt = '封面';
    img.onerror = () => img.remove();
    box.append(img);
  }
  return box;
}

export function initializeFavorites({
  getVideo,
  isLoading,
  notify,
  play,
  favoritesApplication,
  legacyMigration,
}: InitializeFavoritesOptions): FavoritesController {
  const page = requiredElement(
    document.querySelector<HTMLElement>('#profile-page'),
    '收藏夹界面元素不存在',
  );
  const star = requiredElement(
    document.querySelector<HTMLButtonElement>('#favorite-button'),
    '收藏夹界面元素不存在',
  );

  let folders: FavoriteFolder[] = [];
  let screen: Screen = 'home';
  let current: FavoriteFolder | null = null;
  const dialog = el('dialog', '', 'modal');
  document.body.append(dialog);

  async function refresh(): Promise<FavoriteFolder[]> {
    const result = unwrap(await favoritesApplication.list());
    folders = result.folders;
    if (result.errors.length) notify(result.errors.join('\n'), 6000);
    void refreshStar(false);
    return folders;
  }

  async function refreshStar(reload = true): Promise<void> {
    try {
      if (reload) await refresh();
      const saved = folders.some((folder) => folder.items.some((item) => item.bvid === getVideo()?.bvid));
      star.textContent = saved ? '★' : '☆';
      star.classList.toggle('saved', saved);
      star.setAttribute('aria-label', saved ? '已收藏，选择其他收藏夹' : '收藏当前视频');
    } catch (error: unknown) {
      notify(errorMessage(error));
    }
  }

  async function go(next: Screen, id?: string): Promise<void> {
    try {
      await refresh();
      if (id) current = unwrap(await favoritesApplication.get(id));
      screen = next;
      render();
    } catch (error: unknown) {
      notify(errorMessage(error));
    }
  }

  function formDialog<TFields>(
    title: string,
    build: FormBuilder<TFields>,
    submit: FormSubmit<TFields>,
    onClose?: () => void | Promise<void>,
  ): void {
    dialog.replaceChildren();
    dialog.onclose = onClose ? () => { void onClose(); } : null;
    const form = el('form', '', 'modal-card');
    form.append(el('h2', title));
    const fields = build(form);
    const error = el('p', '', 'form-error');
    error.setAttribute('role', 'alert');
    const actions = el('div', '', 'modal-actions');
    const cancel = button('取消', () => dialog.close());
    const confirm = el('button', '确定', 'primary-button');
    confirm.type = 'submit';
    actions.append(cancel, confirm);
    form.append(error, actions);
    dialog.append(form);
    let busy = false;
    dialog.onclick = (event) => {
      if (event.target === dialog && !busy) dialog.close();
    };
    const onCancel = (event: Event): void => {
      if (busy) event.preventDefault();
    };
    dialog.oncancel = onCancel;
    form.onsubmit = async (event): Promise<void> => {
      event.preventDefault();
      if (busy) return;
      busy = true;
      confirm.disabled = true;
      cancel.disabled = true;
      error.textContent = '';
      try {
        await submit(fields);
      } catch (reason: unknown) {
        error.textContent = errorMessage(reason);
      } finally {
        busy = false;
        confirm.disabled = false;
        cancel.disabled = false;
      }
    };
    if (!dialog.open) dialog.showModal();
    form.querySelector<HTMLElement>('input, textarea, select')?.focus();
  }

  interface InputOptions {
    multiline?: boolean;
    maxLength?: number;
    required?: boolean;
  }

  function input(
    form: HTMLFormElement,
    label: string,
    { multiline = false, maxLength, required = true }: InputOptions = {},
  ): HTMLInputElement | HTMLTextAreaElement {
    const wrapper = el('label', label);
    const field = el(multiline ? 'textarea' : 'input');
    field.required = required;
    if (maxLength) field.maxLength = maxLength;
    wrapper.append(field);
    form.append(wrapper);
    return field;
  }

  function create(after?: (id: string) => void | Promise<void>): void {
    formDialog('新建收藏夹', (form) => ({
      name: input(form, '名称', { maxLength: 80 }),
      description: input(form, '描述', { multiline: true, maxLength: 500, required: false }),
    }), async (fields) => {
      const request: FavoriteCreateRequest = {
        name: fields.name.value,
        description: fields.description.value,
      };
      const folder = unwrap(await favoritesApplication.create(request));
      await refresh();
      render();
      dialog.close();
      if (after) await after(folder.id);
    });
  }

  async function choose(selectedId?: string, video = getVideo()): Promise<void> {
    if (!video || isLoading()) return;
    try {
      await refresh();
    } catch (error: unknown) {
      notify(errorMessage(error));
      return;
    }
    formDialog('收藏到', (form) => {
      const list = el('div', '', 'favorite-choices');
      let selected = selectedId;
      for (const folder of folders) {
        const label = el('label', '', 'favorite-choice');
        const radio = el('input');
        radio.type = 'radio';
        radio.name = 'folder';
        radio.value = folder.id;
        radio.required = true;
        radio.checked = selected === folder.id;
        radio.onchange = () => { selected = folder.id; };
        const exists = folder.items.some((item) => item.bvid === video.bvid);
        label.append(radio, cover(folder.items[0]?.coverUrl ?? ''), el('span', `${folder.name} · ${folder.items.length} 个${exists ? ' · 已收藏' : ''}`));
        list.append(label);
      }
      if (!folders.length) list.append(el('p', '还没有收藏夹，请先新建。'));
      form.append(list, button('新建收藏夹', () => create((id) => choose(id, video))));
      return () => selected;
    }, async (selection) => {
      const id = selection();
      if (!id) throw new Error('请选择收藏夹');
      const request: FavoriteAddRequest = { input: video.bvid };
      const result = unwrap(await favoritesApplication.addItem(id, request));
      await refresh();
      if (current?.id === result.folder.id) current = result.folder;
      render();
      dialog.close();
      notify(result.duplicate ? '已在此收藏夹中' : '收藏成功');
    });
  }

  function addVideo(): void {
    const folder = current;
    if (!folder) return;
    formDialog('添加视频', (form) => input(form, 'BV 号或视频链接'), async (field) => {
      const request: FavoriteAddRequest = { input: field.value };
      const result = unwrap(await favoritesApplication.addItem(folder.id, request));
      current = result.folder;
      await refresh();
      render();
      dialog.close();
      notify(result.duplicate ? '视频已在收藏夹中' : '添加成功');
    });
  }

  async function exportFile(
    request: () => Promise<ServiceResult<void>>,
  ): Promise<void> {
    try {
      unwrap(await request());
    } catch (error: unknown) {
      notify(errorMessage(error));
    }
  }

  async function importFile(): Promise<void> {
    try {
      const selection = unwrap(await favoritesApplication.selectImport({
        accept: ['.json', 'application/json'],
        maxBytes: 10 * 1024 * 1024,
      }));
      if (!selection) return;
      formDialog('导入预览', (form) => {
        form.append(el('p', '将创建新收藏夹，不覆盖已有数据。'));
        const list = el('div', '', 'favorite-choices');
        selection.preview.folders.forEach((folder) => list.append(el('p', `${folder.name} · ${folder.items.length} 个视频`)));
        form.append(list);
      }, async () => {
        const result = unwrap(await favoritesApplication.commitImport(selection.data));
        await refresh();
        render();
        dialog.close();
        notify(result.error || `已导入 ${result.folders.length} 个收藏夹`, 6000);
      });
    } catch (error: unknown) {
      notify(errorMessage(error), 5000);
    }
  }

  function migrationStatus(status: string): string {
    switch (status) {
      case 'importable': return '可导入';
      case 'changed': return '源文件已变化';
      case 'migrated': return '已迁移';
      case 'partial': return '部分成功';
      case 'failed': return '导入失败';
      case 'invalid': return '无效文件';
      case 'unsupported': return '不支持的 schema';
      case 'readError': return '读取失败';
      case 'tooLarge': return '超过 10 MB';
      case 'empty': return '没有收藏夹';
      default: return status;
    }
  }

  async function migrateLegacy(): Promise<void> {
    if (!legacyMigration) return;
    try {
      const preview = unwrap(await legacyMigration.select());
      if (!preview) return;
      if (!preview.canCommit) {
        unwrap(await legacyMigration.cancel(preview.sessionId));
        notify(preview.files.length ? '没有可导入的有效收藏夹' : '没有发现 JSON 收藏夹', 5000);
        return;
      }

      let committed = false;
      formDialog(
        '迁移旧收藏夹',
        (form) => {
          form.append(el('p', '只会复制并创建新的收藏夹，不会修改旧目录或覆盖已有数据。'));
          const list = el('div', '', 'favorite-choices');
          for (const file of preview.files) {
            const detail = file.reason ? ` · ${file.reason}` : '';
            list.append(el('p', `${file.name} · ${migrationStatus(file.status)} · ${file.folderCount} 个收藏夹${detail}`));
          }
          form.append(list);
        },
        async () => {
          const result = unwrap(await legacyMigration.commit(preview.sessionId));
          committed = true;
          await refresh();
          render();
          dialog.close();
          notify(migrationNotification(result), 6000);
        },
        () => {
          if (!committed) void legacyMigration.cancel(preview.sessionId);
        },
      );
    } catch (error: unknown) {
      notify(errorMessage(error), 5000);
    }
  }

  function render(): void {
    page.replaceChildren();
    const content = el('div', '', 'favorites-page');
    page.append(content);
    if (screen === 'home') {
      content.append(el('h1', '我的'), button('收藏夹 →', () => go('list'), 'favorite-entry'));
      return;
    }
    const header = el('div', '', 'favorite-toolbar');
    header.append(button('← 返回', () => {
      screen = screen === 'detail' ? 'list' : 'home';
      render();
    }));
    if (screen === 'list') {
      header.append(button('导入', importFile), button('导出全部', () => exportFile(() => favoritesApplication.exportAll('favorites.json'))));
      if (legacyMigration) header.append(button('迁移旧收藏夹', migrateLegacy));
      content.append(header, el('h1', '收藏夹'));
      const list = el('div', '', 'favorite-grid');
      for (const folder of folders) {
        const card = button('', () => go('detail', folder.id), 'favorite-card');
        card.append(cover(folder.items[0]?.coverUrl ?? ''), el('h2', folder.name), el('p', folder.description || '暂无描述'), el('small', `${folder.items.length} 个视频`));
        list.append(card);
      }
      if (!folders.length) list.append(el('p', '还没有收藏夹，点击右下角加号创建。'));
      content.append(list, button('+', () => create(), 'fab'));
      return;
    }

    const folder = current;
    if (!folder) {
      screen = 'list';
      render();
      return;
    }
    header.append(button('导出', () => exportFile(() => favoritesApplication.exportFolder(folder.id, 'favorites.json'))));
    content.append(header, cover(folder.items[0]?.coverUrl ?? ''), el('h1', folder.name), el('p', folder.description), el('p', `${folder.items.length} 个视频`));
    const list = el('div', '', 'favorite-videos');
    for (const item of folder.items) {
      const row = button('', async () => {
        row.disabled = true;
        try {
          await play(folder, item.bvid);
        } finally {
          row.disabled = false;
        }
      }, 'favorite-video');
      const info = el('span');
      info.append(el('strong', item.title), el('small', `${item.owner} · ${item.duration == null ? '未知时长' : Math.floor(item.duration / 60) + ':' + String(Math.floor(item.duration % 60)).padStart(2, '0')}`));
      row.append(cover(item.coverUrl), info);
      list.append(row);
    }
    if (!folder.items.length) list.append(el('p', '收藏夹为空，点击加号添加视频。'));
    content.append(list, button('+', addVideo, 'fab'));
  }

  star.onclick = () => choose();
  render();
  void refreshStar();
  return { refreshStar };
}
