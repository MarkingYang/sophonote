import { Search, SlidersHorizontal } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';

export default function SearchBox() {
  const { searchQuery, setSearchQuery } = useAppStore();

  return (
    <div className="relative">
      <Search size={15} className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
      <input
        type="text"
        value={searchQuery}
        onChange={(e) => setSearchQuery(e.target.value)}
        placeholder="搜索内容..."
        className="input pl-9 pr-10 py-2 text-sm"
      />
      <button className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors">
        <SlidersHorizontal size={14} />
      </button>
    </div>
  );
}
