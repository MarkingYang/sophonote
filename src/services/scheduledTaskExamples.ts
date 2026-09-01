import catalog from '../../examples/scheduled-tasks.json';
import type { HermesCronDraft } from './tauri';

export interface ScheduledTaskExample {
  id: string;
  name: string;
  description: string;
  prompt: string;
  schedule: string;
  skills: string[];
  startPaused: true;
}

function isExample(value: unknown): value is ScheduledTaskExample {
  if (!value || typeof value !== 'object') return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.id === 'string'
    && typeof item.name === 'string'
    && typeof item.description === 'string'
    && typeof item.prompt === 'string'
    && typeof item.schedule === 'string'
    && Array.isArray(item.skills)
    && item.skills.every((skill) => typeof skill === 'string')
    && item.startPaused === true
  );
}

export const scheduledTaskExamples: ScheduledTaskExample[] = catalog.examples.filter(isExample);

export function scheduledTaskExampleDraft(example: ScheduledTaskExample): HermesCronDraft {
  return {
    name: example.name,
    prompt: example.prompt,
    schedule: example.schedule,
    projectId: null,
    skills: [...example.skills],
    provider: null,
    model: null,
    startPaused: true,
  };
}
