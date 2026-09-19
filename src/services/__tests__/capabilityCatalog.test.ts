import { describe, expect, it } from 'vitest';
import { capabilityEntries, filterCapabilityEntries } from '../capabilityCatalog';
import type { EngineCapabilities } from '../tauri';
const host = (engine: EngineCapabilities['engine'], available = true): EngineCapabilities => ({ engine, available, version: null, error: null, managedCategories: [], tools: [{ name: 'read', description: 'read' }], hermes: null });

describe('能力来源与适用引擎', () => {
  it('同名工具在不同引擎下保留独立身份，搜索支持中文、引擎名与多个关键词', () => {
    const rows = [...capabilityEntries(host('pi')), ...capabilityEntries(host('claude_code'))];
    expect(new Set(rows.map((row) => row.id)).size).toBe(2);
    expect(filterCapabilityEntries(rows, 'all', 'tools', 'Claude Code 读取')).toHaveLength(1);
    expect(filterCapabilityEntries(rows, 'pi', 'all', '宿主')).toHaveLength(1);
    expect(filterCapabilityEntries(rows, 'all', 'skills', '')).toHaveLength(0);
  });
  it('运行时校验失败不标记工具为可用', () => {
    expect(capabilityEntries(host('opencode', false))[0].state).toBe('运行时不可用');
  });
  it('目录只暴露 Hermes 返回的技能和 MCP，不据兼容格式推断其它引擎已安装', () => {
    const data = host('hermes');
    data.hermes = { commands: [], references: [], skills: [], tools: [], toolsets: [], mcpServers: [{ name: 'docs', transport: 'http', enabled: true, url: null, command: null, args: [], auth: null, tools: [] }], terminalBackends: { active: '', backends: [] }, hubSources: { indexAvailable: false, sources: [{ id: 'community', label: '社区', available: null, searchable: null, rateLimited: null }] }, browserConnected: false, browserUrl: '' };
    const rows = capabilityEntries(data);
    expect(rows.find((row) => row.category === 'mcp')?.state).toBe('已启用');
    expect(rows.find((row) => row.category === 'hub')?.state).toBe('状态待确认');
    expect(filterCapabilityEntries(rows, 'pi', 'all', '')).toHaveLength(0);
  });
});
