import { beforeEach, describe, expect, it } from 'vitest';
import {
  peekHermesCapabilities,
  peekHermesModelOptions,
  rememberHermesCapabilities,
  rememberHermesModelOptions,
  resetHermesRuntimeCacheForTests,
  resolveStoredHermesSelection,
} from '../hermesRuntimeCache';
import type { HermesCapabilities, HermesModelOptions } from '../tauri';

const options = (over: Partial<HermesModelOptions> = {}): HermesModelOptions => ({
  model: 'kimi',
  provider: 'moonshot',
  providers: [
    { slug: 'moonshot', name: 'Moonshot', models: ['kimi', 'kimi-k2'], authenticated: true, isCurrent: true },
  ],
  ...over,
});

describe('NEXT-004 Hermes 热切换快照', () => {
  beforeEach(() => {
    resetHermesRuntimeCacheForTests();
    window.localStorage.clear();
  });

  it('记住后可在卸载重挂时同步读出', () => {
    expect(peekHermesModelOptions()).toBeNull();
    rememberHermesModelOptions(options());
    expect(peekHermesModelOptions()?.model).toBe('kimi');
    rememberHermesCapabilities({ skills: [{ name: 'markdown' }] } as HermesCapabilities);
    expect(peekHermesCapabilities()?.skills[0]?.name).toBe('markdown');
  });

  it('优先使用 localStorage 里仍有效的模型，否则回退 Runtime 当前值', () => {
    window.localStorage.setItem('sophonote.hermes.provider', 'moonshot');
    window.localStorage.setItem('sophonote.hermes.model', 'kimi-k2');
    expect(resolveStoredHermesSelection(options())).toEqual({ provider: 'moonshot', model: 'kimi-k2' });
    window.localStorage.setItem('sophonote.hermes.model', 'gone');
    expect(resolveStoredHermesSelection(options())).toEqual({ provider: 'moonshot', model: 'kimi' });
  });
});
