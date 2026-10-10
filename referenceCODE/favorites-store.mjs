import { mkdir, readFile, readdir, writeFile, rename, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';

export function invalid(message) { return Object.assign(new Error(message), { status: 400 }); }
function string(value, label, max, required = false) {
  if (typeof value !== 'string' || value.length > max || (required && !value.trim())) throw invalid(`${label}无效`);
  return value.trim();
}
function date(value) {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T/.test(value) || !Number.isFinite(Date.parse(value))) throw invalid('日期无效');
  return value;
}
export function validateFolder(folder) {
  if (!folder || folder.schemaVersion !== 1) throw invalid('不支持的收藏夹版本');
  const name = string(folder.name, '名称', 80, true);
  const description = string(folder.description, '描述', 500);
  if (!Array.isArray(folder.items)) throw invalid('视频列表无效');
  const seen = new Set();
  const items = folder.items.map(item => {
    if (!item || !/^BV[0-9A-Za-z]{10}$/.test(item.bvid)) throw invalid('BV 号无效');
    const coverUrl = string(item.coverUrl, '封面地址', 4096);
    if (coverUrl) {
      let url; try { url = new URL(coverUrl); } catch { throw invalid('封面地址无效'); }
      if (!['https:', 'http:'].includes(url.protocol)) throw invalid('封面地址无效');
    }
    if (item.duration !== null && (!Number.isFinite(item.duration) || item.duration < 0)) throw invalid('时长无效');
    return { bvid: item.bvid, title: string(item.title, '标题', 1000, true), owner: string(item.owner, 'UP 主', 500), duration: item.duration, coverUrl, addedAt: date(item.addedAt) };
  }).filter(item => { if (seen.has(item.bvid)) return false; seen.add(item.bvid); return true; });
  return { schemaVersion: 1, id: folder.id, name, description, createdAt: date(folder.createdAt), updatedAt: date(folder.updatedAt), items };
}
export function validateImport(data) {
  const folders = data?.type === 'bili-sound-favorites' ? data.folders : [data];
  if (data?.schemaVersion !== 1 || !Array.isArray(folders)) throw invalid('导入格式或版本无效');
  return folders.map(validateFolder);
}
export class FavoritesStore {
  constructor(directory) { this.directory = directory; this.locks = new Map(); }
  path(id) {
    if (!/^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(id)) throw invalid('收藏夹 ID 无效');
    return join(this.directory, `${id}.json`);
  }
  async get(id) {
    try { const folder = validateFolder(JSON.parse(await readFile(this.path(id), 'utf8'))); return { ...folder, id }; }
    catch (error) { if (error.code === 'ENOENT') throw Object.assign(new Error('收藏夹不存在'), { status: 404 }); throw error; }
  }
  async list() {
    let names; try { names = await readdir(this.directory); } catch (error) { if (error.code === 'ENOENT') return { folders: [], errors: [] }; throw error; }
    const folders = [], errors = [];
    for (const name of names.filter(name => name.endsWith('.json'))) {
      try { folders.push(await this.get(name.slice(0, -5))); } catch (error) { errors.push(`${name}：${error.message}`); }
    }
    folders.sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
    return { folders, errors };
  }
  async save(folder) {
    await mkdir(this.directory, { recursive: true });
    const path = this.path(folder.id), temporary = `${path}.${randomUUID()}.tmp`;
    try { await writeFile(temporary, JSON.stringify(folder, null, 2), { flag: 'wx' }); await rename(temporary, path); }
    finally { await unlink(temporary).catch(() => {}); }
    return folder;
  }
  async create(name, description = '') {
    const now = new Date().toISOString();
    return this.save(validateFolder({ schemaVersion: 1, id: randomUUID(), name, description, createdAt: now, updatedAt: now, items: [] }));
  }
  async add(id, metadata) {
    const previous = this.locks.get(id) || Promise.resolve();
    const operation = previous.catch(() => {}).then(async () => {
      const folder = await this.get(id);
      if (folder.items.some(item => item.bvid === metadata.bvid)) return { folder, duplicate: true };
      const now = new Date().toISOString();
      folder.items.unshift({ bvid: metadata.bvid, title: metadata.title, owner: metadata.owner || '', duration: metadata.duration ?? null, coverUrl: metadata.coverUrl || '', addedAt: now });
      folder.updatedAt = now;
      await this.save(validateFolder(folder));
      return { folder, duplicate: false };
    });
    this.locks.set(id, operation);
    try { return await operation; } finally { if (this.locks.get(id) === operation) this.locks.delete(id); }
  }
  async import(data, preview = false) {
    const folders = validateImport(data);
    if (preview) return { folders, preview: true };
    const saved = [];
    for (const folder of folders) {
      try { saved.push(await this.save({ ...folder, id: randomUUID() })); }
      catch (error) { return { folders: saved, error: `已导入 ${saved.length} 个收藏夹，后续写入失败：${error.message}` }; }
    }
    return { folders: saved };
  }
}
