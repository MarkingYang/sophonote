import { describe, expect, it } from 'vitest';
import {
  mergeThreadsForProject,
  resolveProjectThreadId,
  type AgentThread,
} from '../agentStore';

function thread(id: string, projectId: string | null): AgentThread {
  return {
    id,
    title: id,
    status: 'completed',
    projectId,
    latestRunId: `run-${id}`,
    createdAt: 1,
    updatedAt: 1,
  };
}

describe('项目 Chat Thread 隔离', () => {
  it('旧项目 activeThreadId 不会被新项目复用', () => {
    const threads = [thread('thread-a', 'project-a'), thread('thread-b', 'project-b')];

    expect(resolveProjectThreadId(threads, 'project-b', 'thread-a')).toBe('thread-b');
    expect(resolveProjectThreadId([threads[0]], 'project-b', 'thread-a')).toBeNull();
  });

  it('项目列表异步返回时只替换自己的 scope', () => {
    const existing = [thread('thread-a', 'project-a'), thread('thread-b-old', 'project-b')];
    const merged = mergeThreadsForProject(
      existing,
      [thread('thread-b-new', 'project-b')],
      'project-b'
    );

    expect(merged.map((item) => item.id)).toEqual(['thread-a', 'thread-b-new']);
  });

  it('全局快捷会话与项目会话严格隔离', () => {
    const threads = [thread('quick-task', null), thread('project-task', 'project-a')];

    expect(resolveProjectThreadId(threads, null, 'project-task')).toBe('quick-task');
    expect(resolveProjectThreadId([threads[1]], null, 'project-task')).toBeNull();

    const merged = mergeThreadsForProject(threads, [thread('quick-task-new', null)]);
    expect(merged.map((item) => item.id)).toEqual(['project-task', 'quick-task-new']);
  });
});
