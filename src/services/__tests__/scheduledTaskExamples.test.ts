import { describe, expect, it } from 'vitest';
import { scheduledTaskExampleDraft, scheduledTaskExamples } from '../scheduledTaskExamples';

describe('scheduled task examples', () => {
  it('publishes five unique, model-free, paused examples', () => {
    expect(scheduledTaskExamples).toHaveLength(5);
    expect(new Set(scheduledTaskExamples.map((item) => item.id)).size).toBe(5);
    expect(new Set(scheduledTaskExamples.map((item) => item.name)).size).toBe(5);

    for (const example of scheduledTaskExamples) {
      const draft = scheduledTaskExampleDraft(example);
      expect(draft.provider).toBeNull();
      expect(draft.model).toBeNull();
      expect(draft.startPaused).toBe(true);
      expect(draft.name).not.toMatch(/mindbox/i);
      expect(draft.prompt).not.toMatch(/mindbox/i);
      expect(draft.skills.every((skill) => skill.startsWith('sophonote-'))).toBe(true);
    }
  });

  it('keeps the former public schedule intents without private runtime metadata', () => {
    expect(scheduledTaskExamples.map((item) => item.schedule)).toEqual([
      '0 20 * * *',
      '30 8 * * *',
      '0 21 * * 0',
      '0 9 1 * *',
      '30 8 * * *',
    ]);
    for (const example of scheduledTaskExamples) {
      expect(Object.keys(example)).not.toEqual(expect.arrayContaining([
        'provider', 'model', 'jobId', 'history', 'output', 'createdAt', 'updatedAt',
      ]));
    }
  });
});
