// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AgentEngineControl } from '../AgentEngineControl';

let root: Root;
let container: HTMLDivElement;
const select = vi.fn(async () => true);
const trigger = () => container.querySelector<HTMLButtonElement>('[aria-label="智能体引擎"]')!;
const choice = () => [...container.querySelectorAll('button')].find((button) => button.textContent?.includes('Pi'))!;
async function render() {
  await act(async () => root.render(createElement(AgentEngineControl, { value: 'hermes', onSelect: select })));
  await act(async () => trigger().click());
}

describe('引擎切换入口', () => {
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    select.mockReset().mockResolvedValue(true);
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); });
  it('入口保持可点击，选择 Pi 后关闭菜单', async () => {
    await render();
    expect(trigger().disabled).toBe(false);
    expect(choice().textContent).toBe('Pi');
    await act(async () => choice().click());
    expect(select).toHaveBeenCalledWith('pi');
    expect(trigger().getAttribute('aria-expanded')).toBe('false');
  });
  it('菜单仅显示三个引擎名称，并标记当前选项', async () => {
    await render();
    expect(choice().textContent).toBe('Pi');
    const menu = container.querySelector('[aria-label="选择智能体引擎"]')!;
    expect(menu.textContent).toBe('HermesPiClaude Code');
    expect(menu.querySelector('[aria-pressed="true"]')?.textContent).toBe('Hermes');
    await act(async () => choice().click());
    expect(select).toHaveBeenCalledOnce();
  });
  it('创建失败保留当前引擎和菜单，允许重试', async () => {
    select.mockResolvedValue(false);
    await render();
    await act(async () => choice().click());
    expect(trigger().textContent).toBe('Hermes');
    expect(trigger().getAttribute('aria-expanded')).toBe('true');
    expect(choice().disabled).toBe(false);
  });
  it('等待新会话期间阻止重复请求', async () => {
    let resolve!: (value: boolean) => void;
    select.mockImplementation(() => new Promise<boolean>((done) => { resolve = done; }));
    await render();
    await act(async () => choice().click());
    expect(choice().disabled).toBe(true);
    expect(trigger().disabled).toBe(true);
    await act(async () => { resolve(true); });
    expect(select).toHaveBeenCalledOnce();
    expect(trigger().disabled).toBe(false);
  });
  it('Escape 关闭菜单并恢复焦点', async () => {
    await render();
    await act(async () => choice().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
    expect(trigger().getAttribute('aria-expanded')).toBe('false');
    expect(document.activeElement).toBe(trigger());
    expect(select).not.toHaveBeenCalled();
  });
  it('can select Claude Code from Pi without disabling the engine menu', async () => {
    await act(async () => root.render(createElement(AgentEngineControl, { value: 'pi', onSelect: select })));
    await act(async () => trigger().click());
    const claude = [...container.querySelectorAll('button')].find((button) => button.textContent === 'Claude Code')!;
    expect(claude.disabled).toBe(false);
    await act(async () => claude.click());
    expect(select).toHaveBeenCalledWith('claude_code');
    expect(trigger().getAttribute('aria-expanded')).toBe('false');
  });

});
