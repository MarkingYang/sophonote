import type { EngineCapabilities } from './tauri';

export type CapabilityEngine = EngineCapabilities['engine'];
export type CapabilityCategory = EngineCapabilities['managedCategories'][number];
export const CAPABILITY_ENGINES = [
  { id: 'pi', label: 'Pi' },
  { id: 'claude_code', label: 'Claude Code' },
  { id: 'opencode', label: 'OpenCode' },
  { id: 'hermes', label: 'Hermes' },
] as const;
export const CAPABILITY_CATEGORIES = [
  { id: 'skills', label: '技能' },
  { id: 'tools', label: '工具' },
  { id: 'mcp', label: 'MCP' },
  { id: 'hub', label: '资源广场' },
] as const;

export interface CapabilityEntry {
  id: string;
  engine: CapabilityEngine;
  category: CapabilityCategory;
  name: string;
  description: string;
  source: string;
  state: string;
  selection: string;
}

const HOST_DESCRIPTIONS: Record<string, string> = {
  read: '读取授权工作区或显式附件中的文本文件。',
  ls: '查看授权目录中的文件。',
  write: '通过宿主权限检查写入文件；笔记工作副本提交为待审阅修改。',
  edit: '精确替换一处文本；匹配缺失或不唯一时停止。',
  bash: '经用户批准在授权工作区执行命令，受沙箱与超时限制。',
};

export function capabilityEntries(snapshot: EngineCapabilities): CapabilityEntry[] {
  const entries: CapabilityEntry[] = [];
  const add = (category: CapabilityCategory, name: string, description: string, source: string, state: string, selection = name, identity = name) => {
    entries.push({ id: JSON.stringify([snapshot.engine, category, identity]), engine: snapshot.engine, category, name, description, source, state: snapshot.available ? state : '运行时不可用', selection });
  };
  const native = snapshot.hermes;
  if (native) {
    native.skills.forEach((skill) => add('skills', skill.name, skill.description, skill.origin || skill.provenance || 'Hermes Runtime', skill.enabled !== false ? '已启用' : '已停用'));
    native.tools.forEach((tool) => {
      const group = native.toolsets.find((set) => set.tools.includes(tool.name));
      add('tools', tool.name, tool.description, 'Hermes Runtime', group ? (group.enabled ? '已启用' : '已停用') : '已注册', group?.name ?? tool.name);
    });
    native.toolsets.filter((set) => !native.tools.some((tool) => set.tools.includes(tool.name)))
      .forEach((set) => add('tools', set.name, set.description, 'Hermes Runtime', set.enabled ? '已启用' : '已停用', set.name, `toolset:${set.name}`));
    native.mcpServers.forEach((server) => add('mcp', server.name, `${server.transport} · ${server.tools.length} 个工具`, 'Hermes MCP 配置', server.enabled ? '已启用' : '已停用'));
    native.hubSources.sources.forEach((source) => add('hub', source.label, '搜索、预览技能并安装到 Hermes。', source.id, source.rateLimited ? '请求受限' : source.available === false ? '来源不可用' : source.available === true ? '来源可用' : '状态待确认', source.label, source.id));
  } else {
    snapshot.tools.forEach((tool) => add('tools', tool.name, HOST_DESCRIPTIONS[tool.name] ?? tool.description, 'SophoNote 宿主', '按会话权限可用'));
  }
  return entries;
}

export function filterCapabilityEntries(entries: CapabilityEntry[], engine: CapabilityEngine | 'all', category: CapabilityCategory | 'all', query: string): CapabilityEntry[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return entries.filter((entry) => (engine === 'all' || entry.engine === engine)
    && (category === 'all' || entry.category === category)
    && words.every((word) => [entry.name, entry.description, entry.source, entry.engine, CAPABILITY_ENGINES.find((item) => item.id === entry.engine)?.label ?? '', CAPABILITY_CATEGORIES.find((item) => item.id === entry.category)?.label ?? ''].join(' ').toLocaleLowerCase().includes(word)));
}
