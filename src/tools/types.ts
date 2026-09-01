import type { LucideIcon } from 'lucide-react';
import type { ComponentType } from 'react';

/**
 * 工具组件库（DEC-041）：工具域 = 注册式组件库。
 * 每个工具是自包含的整页组件 + 展示元数据；新增工具只需在 registry 注册。
 */

/** 工具分类：随工具增长扩展（日历、提醒、闹钟进入时新增） */
export type ToolCategory = '行动管理' | '专注节奏';

export interface ToolDefinition {
  /** 全局唯一 id；导航状态与测试断言都依赖它 */
  id: string;
  /** 工具名（画廊卡片与页头展示） */
  title: string;
  /** 一句话说明（画廊卡片展示） */
  description: string;
  icon: LucideIcon;
  category: ToolCategory;
  /** 画廊检索关键词 */
  keywords: string[];
  /** 工具整页组件：自包含、无必填 props，渲染自己的滚动容器 */
  Component: ComponentType;
}
