import { CalendarCheck2, ListTodo, Timer } from 'lucide-react';
import type { ToolDefinition } from './types';
import TodayTool from './TodayTool';
import TasksTool from './TasksTool';
import PomodoroTool from './PomodoroTool';

/**
 * 工具注册表（DEC-041）：新增工具 = 新建整页组件 + 在此追加一条 ToolDefinition。
 * 画廊渲染、页头标题、导航状态全部由本表驱动，壳层不硬编码任何工具。
 * id 唯一性由单测（tools/__tests__/registry.test.ts）保证。
 */
export const toolRegistry: ToolDefinition[] = [
  {
    id: 'today',
    title: '今日',
    description: '今天该做什么：逾期、今日到期、今日完成一屏看清',
    icon: CalendarCheck2,
    category: '行动管理',
    keywords: ['今日视图', '待办', '截止', '逾期'],
    Component: TodayTool,
  },
  {
    id: 'tasks',
    title: '任务',
    description: '全量待办管理：笔记任务清单 + 独立任务',
    icon: ListTodo,
    category: '行动管理',
    keywords: ['任务', '清单', '笔记任务', 'todo'],
    Component: TasksTool,
  },
  {
    id: 'pomodoro',
    title: '番茄钟',
    description: '任务关联番茄工作法：25 分钟专注，记录汇入统计',
    icon: Timer,
    category: '专注节奏',
    keywords: ['番茄', '专注', '计时', '休息'],
    Component: PomodoroTool,
  },
];

export function getTool(id: string | null): ToolDefinition | undefined {
  if (!id) return undefined;
  return toolRegistry.find((t) => t.id === id);
}
