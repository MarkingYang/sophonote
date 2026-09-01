/**
 * NEXT-001 性能夹具探针注册表：场景运行器（perfRunner）需要拿到当前活跃的
 * 编辑器与工作台句柄，但它们在组件树深处（NoteWorkbench → MarkdownEditor）。
 * 这里提供一个极薄的模块级持有层：组件挂载时注册、销毁时注销（仅当仍指向
 * 同一实例时才清除，避免新旧实例交替挂载时误清）。
 *
 * 注意：只做类型导入，运行时无组件依赖，不构成循环。
 */

import type { MarkdownEditorHandle } from '../components/editor/MarkdownEditor';
import type { NoteWorkbenchHandle } from '../components/features/NoteWorkbench';

export interface PerfProbeTargets {
  editor: MarkdownEditorHandle | null;
  workbench: NoteWorkbenchHandle | null;
}

let current: PerfProbeTargets = { editor: null, workbench: null };
const listeners = new Set<() => void>();

function emit(): void {
  listeners.forEach((l) => l());
}

export function getPerfProbeTargets(): PerfProbeTargets {
  return current;
}

export function subscribePerfProbeTargets(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** 注册某一槽位；返回注销函数（只清除仍等于本实例的槽位） */
export function registerPerfProbeTarget<K extends keyof PerfProbeTargets>(
  key: K,
  value: NonNullable<PerfProbeTargets[K]>,
): () => void {
  current = { ...current, [key]: value };
  emit();
  return () => {
    if (current[key] === value) {
      current = { ...current, [key]: null };
      emit();
    }
  };
}
