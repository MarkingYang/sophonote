/**
 * NB-31 测试基座：全局 Storage 垫片（setupFiles，先于测试文件加载执行）。
 *
 * 背景（Node ≥22.4 与 vitest jsdom 的冲突）：
 * - Node ≥22.4 自带实验性 localStorage/sessionStorage 全局——未提供 --localstorage-file
 *   时访问即抛错（并打印 ExperimentalWarning）；
 * - vitest jsdom 环境把 window 属性拷到 globalThis 时，会跳过「globalThis 已存在
 *   且不在其内置白名单」的键（environments populateGlobal/getWindowKeys 逻辑），
 *   localStorage 不在白名单 → jsdom 的可用实现永远不会覆盖 Node 的坏全局；
 * - zustand persist 默认 storage = createJSONStorage(() => window.localStorage)，
 *   vitest jsdom 里 window === globalThis，于是拿到的是 Node 的坏全局：
 *   createJSONStorage 读到 undefined（不抛错）后返回一个内部 storage 为 undefined
 *   的对象 → 任何 setState 触发「Cannot read properties of undefined (reading 'setItem')」。
 *
 * 修法：在测试文件加载前用内存 Storage 显式覆盖 globalThis.localStorage/sessionStorage。
 * 对测试而言持久层只需可读写的内存实现（appStore 的 persist 写入不影响断言），
 * 确定性优于依赖 jsdom/Node 各版本的全局注入行为。
 */

function createMemoryStorage(): Storage {
  const store = new Map<string, string>();
  return {
    getItem: (key: string) => (store.has(key) ? (store.get(key) as string) : null),
    setItem: (key: string, value: string) => { store.set(String(key), String(value)); },
    removeItem: (key: string) => { store.delete(String(key)); },
    clear: () => { store.clear(); },
    key: (index: number) => [...store.keys()][index] ?? null,
    get length() { return store.size; },
  };
}

// Node 的实验性全局是 configurable 的，defineProperty 可安全覆盖（已实测）。
Object.defineProperty(globalThis, 'localStorage', {
  value: createMemoryStorage(),
  configurable: true,
  writable: true,
});
Object.defineProperty(globalThis, 'sessionStorage', {
  value: createMemoryStorage(),
  configurable: true,
  writable: true,
});
