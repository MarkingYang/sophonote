import { Star, Cpu, Layers, BarChart3 } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { Collection } from '../../types';

// 内置收藏夹的商务风格线性图标（与主导航同一图标体系，单色继承文字色）
const COLLECTION_ICONS: Record<string, LucideIcon> = {
  favorites: Star,
  'ai-models': Cpu,
  architecture: Layers,
  products: BarChart3,
};

export default function CollectionIcon({
  collection,
  size = 15,
}: {
  collection: Collection;
  size?: number;
}) {
  const Icon = COLLECTION_ICONS[collection.id];
  if (!Icon) {
    // 自定义收藏夹：沿用创建时选择的 emoji
    return <span style={{ color: collection.color }}>{collection.icon}</span>;
  }
  return <Icon size={size} className="shrink-0" />;
}
