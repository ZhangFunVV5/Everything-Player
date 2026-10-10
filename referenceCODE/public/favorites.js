async function api(path = '', options = {}) {
  const response = await fetch(`/api/favorites${path}`, options.body === undefined ? options : {
    ...options, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(options.body),
  });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || '收藏夹操作失败');
  return result;
}
function el(tag, text, className) {
  const node = document.createElement(tag);
  if (text) node.textContent = text;
  if (className) node.className = className;
  return node;
}
function button(text, action, className = 'secondary-button') {
  const node = el('button', text, className); node.type = 'button'; node.onclick = action; return node;
}
function cover(url) {
  const box = el('span', '♪', 'favorite-cover');
  if (url) { const img = el('img'); img.src = url; img.referrerPolicy = 'no-referrer'; img.alt = '封面'; img.onerror = () => img.remove(); box.append(img); }
  return box;
}
export function initializeFavorites({ getVideo, isLoading, notify, play }) {
  const page = document.querySelector('#profile-page');
  const star = document.querySelector('#favorite-button');
  let folders = [], screen = 'home', current = null;
  const dialog = el('dialog', '', 'modal'); document.body.append(dialog);

  async function refresh() {
    const result = await api(); folders = result.folders;
    if (result.errors.length) notify(result.errors.join('\n'), 6000);
    refreshStar(false);
    return folders;
  }
  async function refreshStar(reload = true) {
    try {
      if (reload) await refresh();
      const saved = folders.some(folder => folder.items.some(item => item.bvid === getVideo()?.bvid));
      star.textContent = saved ? '★' : '☆'; star.classList.toggle('saved', saved);
      star.setAttribute('aria-label', saved ? '已收藏，选择其他收藏夹' : '收藏当前视频');
    } catch (error) { notify(error.message); }
  }
  async function go(next, id) {
    try { await refresh(); if (id) current = await api(`/${id}`); screen = next; render(); }
    catch (error) { notify(error.message); }
  }
  function formDialog(title, build, submit) {
    dialog.replaceChildren(); const form = el('form', '', 'modal-card');
    form.append(el('h2', title)); const fields = build(form);
    const error = el('p', '', 'form-error'); error.setAttribute('role', 'alert');
    const actions = el('div', '', 'modal-actions');
    const cancel = button('取消', () => dialog.close());
    const confirm = el('button', '确定', 'primary-button'); confirm.type = 'submit';
    actions.append(cancel, confirm); form.append(error, actions); dialog.append(form);
    let busy = false;
    dialog.onclick = event => { if (event.target === dialog && !busy) dialog.close(); };
    const onCancel = event => { if (busy) event.preventDefault(); };
    dialog.oncancel = onCancel;
    form.onsubmit = async event => {
      event.preventDefault(); if (busy) return; busy = true; confirm.disabled = true; cancel.disabled = true; error.textContent = '';
      try { await submit(fields); }
      catch (reason) { error.textContent = reason.message; }
      finally { busy = false; confirm.disabled = false; cancel.disabled = false; }
    };
    if (!dialog.open) dialog.showModal();
    form.querySelector('input, textarea, select')?.focus();
  }
  function input(form, label, { multiline = false, maxLength, required = true } = {}) {
    const wrapper = el('label', label); const field = el(multiline ? 'textarea' : 'input');
    field.required = required; if (maxLength) field.maxLength = maxLength;
    wrapper.append(field); form.append(wrapper); return field;
  }
  function create(after) {
    formDialog('新建收藏夹', form => ({
      name: input(form, '名称', { maxLength: 80 }),
      description: input(form, '描述', { multiline: true, maxLength: 500, required: false }),
    }), async fields => {
      const folder = await api('', { method: 'POST', body: { name: fields.name.value, description: fields.description.value } });
      await refresh(); render(); dialog.close(); if (after) await after(folder.id);
    });
  }
  async function choose(selectedId, video = getVideo()) {
    if (!video || isLoading()) return;
    try { await refresh(); } catch (error) { notify(error.message); return; }
    formDialog('收藏到', form => {
      const list = el('div', '', 'favorite-choices'); let selected = selectedId;
      for (const folder of folders) {
        const label = el('label', '', 'favorite-choice'); const radio = el('input');
        radio.type = 'radio'; radio.name = 'folder'; radio.value = folder.id; radio.required = true; radio.checked = selected === folder.id;
        radio.onchange = () => { selected = folder.id; };
        const exists = folder.items.some(item => item.bvid === video.bvid);
        label.append(radio, cover(folder.items[0]?.coverUrl), el('span', `${folder.name} · ${folder.items.length} 个${exists ? ' · 已收藏' : ''}`)); list.append(label);
      }
      if (!folders.length) list.append(el('p', '还没有收藏夹，请先新建。'));
      form.append(list, button('新建收藏夹', () => create(id => choose(id, video))));
      return () => selected;
    }, async selection => {
      if (!selection()) throw new Error('请选择收藏夹');
      const result = await api(`/${selection()}/items`, { method: 'POST', body: { input: video.bvid } });
      await refresh(); if (current?.id === result.folder.id) current = result.folder; render(); dialog.close(); notify(result.duplicate ? '已在此收藏夹中' : '收藏成功');
    });
  }
  function addVideo() {
    const id = current.id;
    formDialog('添加视频', form => input(form, 'BV 号或视频链接'), async field => {
      const result = await api(`/${id}/items`, { method: 'POST', body: { input: field.value } });
      current = result.folder; await refresh(); render(); dialog.close(); notify(result.duplicate ? '视频已在收藏夹中' : '添加成功');
    });
  }
  async function exportFile(path) {
    try {
      const data = await api(path); const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }));
      const link = el('a'); link.href = url; link.download = 'favorites.json'; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (error) { notify(error.message); }
  }
  function importFile() {
    const picker = el('input'); picker.type = 'file'; picker.accept = '.json,application/json';
    picker.onchange = async () => {
      try {
        const file = picker.files[0]; if (!file) return;
        if (file.size > 10 * 1024 * 1024) throw new Error('文件不能超过 10 MB');
        let data; try { data = JSON.parse(await file.text()); } catch { throw new Error('文件不是有效 JSON'); }
        const preview = await api('/import', { method: 'POST', body: { data, preview: true } });
        formDialog('导入预览', form => {
          form.append(el('p', '将创建新收藏夹，不覆盖已有数据。'));
          const list = el('div', '', 'favorite-choices');
          preview.folders.forEach(folder => list.append(el('p', `${folder.name} · ${folder.items.length} 个视频`))); form.append(list);
        }, async () => {
          const result = await api('/import', { method: 'POST', body: { data } });
          await refresh(); render(); dialog.close(); notify(result.error || `已导入 ${result.folders.length} 个收藏夹`, 6000);
        });
      } catch (error) { notify(error.message, 5000); }
    };
    picker.click();
  }
  function render() {
    page.replaceChildren(); const content = el('div', '', 'favorites-page'); page.append(content);
    if (screen === 'home') { content.append(el('h1', '我的'), button('收藏夹 →', () => go('list'), 'favorite-entry')); return; }
    const header = el('div', '', 'favorite-toolbar');
    header.append(button('← 返回', () => { screen = screen === 'detail' ? 'list' : 'home'; render(); }));
    if (screen === 'list') {
      header.append(button('导入', importFile), button('导出全部', () => exportFile('/export')));
      content.append(header, el('h1', '收藏夹'));
      const list = el('div', '', 'favorite-grid');
      for (const folder of folders) {
        const card = button('', () => go('detail', folder.id), 'favorite-card');
        card.append(cover(folder.items[0]?.coverUrl), el('h2', folder.name), el('p', folder.description || '暂无描述'), el('small', `${folder.items.length} 个视频`)); list.append(card);
      }
      if (!folders.length) list.append(el('p', '还没有收藏夹，点击右下角加号创建。'));
      content.append(list, button('+', () => create(), 'fab'));
    } else {
      header.append(button('导出', () => exportFile(`/${current.id}/export`)));
      content.append(header, cover(current.items[0]?.coverUrl), el('h1', current.name), el('p', current.description), el('p', `${current.items.length} 个视频`));
      const list = el('div', '', 'favorite-videos');
      for (const item of current.items) {
        const row = button('', async () => { row.disabled = true; try { await play(current, item.bvid); } finally { row.disabled = false; } }, 'favorite-video');
        const info = el('span'); info.append(el('strong', item.title), el('small', `${item.owner} · ${item.duration == null ? '未知时长' : Math.floor(item.duration / 60) + ':' + String(Math.floor(item.duration % 60)).padStart(2, '0')}`));
        row.append(cover(item.coverUrl), info); list.append(row);
      }
      if (!current.items.length) list.append(el('p', '收藏夹为空，点击加号添加视频。'));
      content.append(list, button('+', addVideo, 'fab'));
    }
  }
  star.onclick = () => choose(); render(); refreshStar();
  return { refreshStar };
}
