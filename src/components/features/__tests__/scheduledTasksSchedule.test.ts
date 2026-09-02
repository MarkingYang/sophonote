import { describe, expect, it } from 'vitest';
import type { HermesCronJobInfo, HermesCronRunInfo, HermesModelOptions } from '../../../services/tauri';
import { scheduledTaskExampleDraft, scheduledTaskExamples } from '../../../services/scheduledTaskExamples';
import {
  editorFromDraft,
  editorFromJob,
  editorRule,
  hasActiveCronRun,
  isCronRunStartupStalled,
  modelValueIsAvailable,
  scheduleFromEditor,
  selectableModelProviders,
  taskHasConfiguredModel,
  taskRule,
  type EditorState,
} from '../ScheduledTasksPanel';

const editor: EditorState = {
  name: '任务',
  prompt: '执行任务',
  projectId: '',
  frequency: 'daily',
  time: '09:30',
  weekday: '1',
  intervalValue: '2',
  intervalUnit: 'h',
  onceAt: '2026-08-20T15:00',
  customSchedule: '15 8 1 * *',
  skills: [],
  modelValue: '',
};

function job(expr: string): HermesCronJobInfo {
  return {
    id: 'job-1',
    name: '任务',
    prompt: '执行任务',
    schedule: expr,
    scheduleKind: 'cron',
    scheduleSpec: { kind: 'cron', expr },
    status: 'active',
    enabled: true,
    nextRunAt: '2026-08-16T09:00:00+08:00',
    lastRunAt: null,
    lastStatus: null,
    lastError: null,
    skills: [],
    profile: 'default',
    executionStatus: null,
    createdAt: '2026-08-15T09:00:00+08:00',
    projectId: null,
    projectName: null,
    provider: null,
    model: null,
  };
}

describe('计划任务常用频率', () => {
  it('生成 Claude Desktop 同层级的常用计划', () => {
    expect(scheduleFromEditor({ ...editor, frequency: 'hourly' })).toBe('0 * * * *');
    expect(scheduleFromEditor({ ...editor, frequency: 'daily' })).toBe('30 9 * * *');
    expect(scheduleFromEditor({ ...editor, frequency: 'weekdays' })).toBe('30 9 * * 1-5');
    expect(scheduleFromEditor({ ...editor, frequency: 'weekly', weekday: '3' })).toBe('30 9 * * 3');
  });

  it('保留 Hermes 的间隔、单次与高级计划', () => {
    expect(scheduleFromEditor({ ...editor, frequency: 'interval' })).toBe('every 2h');
    expect(scheduleFromEditor({ ...editor, frequency: 'once' })).toBe('2026-08-20T15:00');
    expect(scheduleFromEditor({ ...editor, frequency: 'custom' })).toBe('15 8 1 * *');
  });

  it('把常见 cron 投影为中文规则并可回填编辑器', () => {
    expect(taskRule(job('0 * * * *'))).toBe('每小时整点');
    expect(taskRule(job('0 9 * * 1-5'))).toBe('工作日 09:00');
    expect(taskRule(job('30 14 * * 5'))).toBe('每周五 14:30');
    expect(editorFromJob(job('0 9 * * 1-5')).frequency).toBe('weekdays');
    expect(editorFromJob(job('15 * * * 1')).frequency).toBe('custom');
  });

  it('把公开范例回填为可审阅表单且不添加模型', () => {
    const daily = editorFromDraft(scheduledTaskExampleDraft(scheduledTaskExamples[0]));
    const monthly = editorFromDraft(scheduledTaskExampleDraft(scheduledTaskExamples[3]));
    expect(daily).toMatchObject({ name: '每日高质量发现', frequency: 'daily', time: '20:00', modelValue: '' });
    expect(monthly).toMatchObject({ frequency: 'custom', customSchedule: '0 9 1 * *', modelValue: '' });
  });

  it('空模型保持未配置，只有 provider/model 成对出现才可运行', () => {
    const empty = job('0 9 * * *');
    expect(editorFromJob(empty).modelValue).toBe('');
    expect(taskHasConfiguredModel(empty)).toBe(false);
    expect(taskHasConfiguredModel({ ...empty, provider: 'deepseek', model: null })).toBe(false);
    expect(taskHasConfiguredModel({ ...empty, provider: 'deepseek', model: 'deepseek-v4-flash' })).toBe(true);
  });

  it('Runtime 重连后的模型目录可恢复原计划任务模型', () => {
    const unavailable: HermesModelOptions = { provider: null, model: null, providers: [] };
    const recovered: HermesModelOptions = {
      provider: 'deepseek',
      model: 'deepseek-v4-flash',
      providers: [{
        slug: 'deepseek',
        name: 'DeepSeek',
        models: ['deepseek-v4-flash'],
        authenticated: true,
        isCurrent: true,
      }],
    };
    const value = JSON.stringify(['deepseek', 'deepseek-v4-flash']);
    expect(modelValueIsAvailable(unavailable, value)).toBe(false);
    expect(selectableModelProviders(unavailable)).toHaveLength(0);
    expect(modelValueIsAvailable(recovered, value)).toBe(true);
    expect(selectableModelProviders(recovered)).toHaveLength(1);
  });

  it('表单始终提供本地化可读摘要', () => {
    expect(editorRule({ ...editor, frequency: 'hourly' })).toBe('每小时整点执行');
    expect(editorRule({ ...editor, frequency: 'weekdays' })).toBe('每个工作日 09:30');
  });

  it('运行未结束时锁定再次触发', () => {
    const run = (status: HermesCronRunInfo['status']): HermesCronRunInfo => ({
      sessionId: 'run-1',
      status,
      startedAt: 1,
      endedAt: null,
      preview: '',
      endReason: null,
      profile: 'default',
      model: 'deepseek-v4-flash',
      toolCallCount: 2,
      modelCallCount: 1,
      lastActivity: null,
    });
    expect(hasActiveCronRun([run('running')])).toBe(true);
    expect(hasActiveCronRun([run('pending')])).toBe(true);
    expect(hasActiveCronRun([run('completed')])).toBe(false);
    expect(hasActiveCronRun([run('error')])).toBe(false);
  });

  it('首轮两分钟无模型和工具完成记录时标记启动异常', () => {
    const run: HermesCronRunInfo = {
      sessionId: 'run-stalled',
      status: 'running',
      startedAt: 1_000,
      endedAt: null,
      preview: '',
      endReason: null,
      profile: 'default',
      model: 'deepseek-v4-flash',
      toolCallCount: 0,
      modelCallCount: 0,
      lastActivity: 'waiting for non-streaming API response',
    };
    expect(isCronRunStartupStalled(run, 1_119)).toBe(false);
    expect(isCronRunStartupStalled(run, 1_120)).toBe(true);
    expect(isCronRunStartupStalled({ ...run, modelCallCount: 1 }, 1_120)).toBe(false);
    expect(isCronRunStartupStalled({ ...run, toolCallCount: 1 }, 1_120)).toBe(false);
    expect(isCronRunStartupStalled({ ...run, status: 'completed' }, 1_120)).toBe(false);
  });
});
