import { useEffect, useMemo, useState } from 'react';
import { Loader2 } from 'lucide-react';
import * as tauri from '../../services/tauri';

const GROUPS = ['公司与模型', '技术方向', '内容形态'] as const;

export default function DiscoverTopics({ onSelect }: { onSelect: (topic: string) => void }) {
  const [topics, setTopics] = useState<tauri.DiscoveryTopicSummary[]>([]);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    tauri.getDiscoveryTopicsSummary()
      .then(setTopics)
      .catch((error) => console.error('Failed to load discovery topics:', error))
      .finally(() => setLoading(false));
  }, []);
  const grouped = useMemo(
    () => GROUPS.map((group) => ({ group, rows: topics.filter((topic) => topic.group === group) })),
    [topics],
  );
  if (loading) return <div className="hb-d-state-wrap py-24"><Loader2 className="animate-spin" size={18} /></div>;
  return (
    <div className="hb-d-section-shell hb-d-topics-shell">
      {grouped.map(({ group, rows }) => (
        <section key={group} className="hb-d-topics-group">
          <div className="hb-d-topics-group-head">
            <h4 className="hb-d-topics-group-title">{group}</h4>
          </div>
          <div className="hb-d-topic-grid">
            {rows.map((topic) => (
              <button
                key={topic.name}
                onClick={() => onSelect(topic.name)}
                className="hb-d-topic-card"
              >
                <span className="hb-d-topic-name">{topic.name}</span>
                <span className="hb-d-topic-count">{topic.count} 条</span>
              </button>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
