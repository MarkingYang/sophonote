import ScheduledTasksPanel from '../components/features/ScheduledTasksPanel';

export default function ScheduledTasks() {
  return (
    <div className="flex h-full min-w-0 flex-col bg-[var(--bg-canvas)]">
      <header
        className="flex h-10 shrink-0 items-center border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-5"
        data-tauri-drag-region
      >
        <h2 className="text-base font-semibold text-[var(--text-primary)]" data-tauri-drag-region>
          计划任务
        </h2>
      </header>
      <main className="flex-1 overflow-y-auto p-4">
        <ScheduledTasksPanel />
      </main>
    </div>
  );
}
