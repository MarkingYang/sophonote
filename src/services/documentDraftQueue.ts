import { flushDocument } from './noteSave';

export interface DocumentDraftSeed {
  documentId: string;
  markdown: string;
  title: string;
}

export interface DocumentDraftWriters {
  writeContent: (documentId: string, markdown: string) => Promise<void>;
  writeTitle: (documentId: string, title: string) => Promise<void>;
}

export interface DocumentDraftSnapshot extends DocumentDraftSeed {
  dirty: boolean;
  saving: boolean;
  error: string | null;
  generation: number;
  persistedGeneration: number;
}

interface DocumentDraftRecord extends DocumentDraftSeed {
  savedMarkdown: string;
  savedTitle: string;
  generation: number;
  persistedGeneration: number;
  error: string | null;
  inFlight: Promise<boolean> | null;
}

/**
 * 按 documentId 隔离的草稿与保存队列。
 *
 * - 同文档只允许一个保存循环；保存期间产生的新 generation 会在同一循环继续落盘。
 * - 不同文档互不共享 inFlight/baseline，旧文档完成回调不会污染新文档。
 * - 队列位于 React 生命周期之外，页签卸载后已捕获的草稿仍能完成写入。
 */
export class DocumentDraftQueue {
  private readonly records = new Map<string, DocumentDraftRecord>();

  seed(seed: DocumentDraftSeed): DocumentDraftSnapshot {
    const existing = this.records.get(seed.documentId);
    if (!existing) {
      const record: DocumentDraftRecord = {
        ...seed,
        savedMarkdown: seed.markdown,
        savedTitle: seed.title,
        generation: 0,
        persistedGeneration: 0,
        error: null,
        inFlight: null,
      };
      this.records.set(seed.documentId, record);
      return this.snapshotOf(record);
    }

    // 只有完全干净时才接受 store/磁盘的新基线；失败或在途草稿不能被重挂载覆盖。
    if (!this.isDirty(existing) && !existing.inFlight) {
      existing.markdown = seed.markdown;
      existing.title = seed.title;
      existing.savedMarkdown = seed.markdown;
      existing.savedTitle = seed.title;
      existing.error = null;
    }
    return this.snapshotOf(existing);
  }

  update(seed: DocumentDraftSeed): DocumentDraftSnapshot {
    const record = this.records.get(seed.documentId) ?? this.create(seed);
    if (record.markdown !== seed.markdown || record.title !== seed.title) {
      record.markdown = seed.markdown;
      record.title = seed.title;
      record.generation += 1;
      record.error = null;
    }
    return this.snapshotOf(record);
  }

  markPersisted(seed: DocumentDraftSeed): DocumentDraftSnapshot {
    const record = this.records.get(seed.documentId) ?? this.create(seed);
    record.markdown = seed.markdown;
    record.title = seed.title;
    record.savedMarkdown = seed.markdown;
    record.savedTitle = seed.title;
    record.persistedGeneration = record.generation;
    record.error = null;
    return this.snapshotOf(record);
  }

  get(documentId: string): DocumentDraftSnapshot | null {
    const record = this.records.get(documentId);
    return record ? this.snapshotOf(record) : null;
  }

  flush(documentId: string, writers: DocumentDraftWriters): Promise<boolean> {
    const record = this.records.get(documentId);
    if (!record) return Promise.resolve(true);
    if (record.inFlight) return record.inFlight;

    const pending = this.run(record, writers);
    record.inFlight = pending;
    void pending.finally(() => {
      if (record.inFlight === pending) record.inFlight = null;
    });
    return pending;
  }

  remove(documentId: string): void {
    this.records.delete(documentId);
  }

  private create(seed: DocumentDraftSeed): DocumentDraftRecord {
    const record: DocumentDraftRecord = {
      ...seed,
      savedMarkdown: seed.markdown,
      savedTitle: seed.title,
      generation: 0,
      persistedGeneration: 0,
      error: null,
      inFlight: null,
    };
    this.records.set(seed.documentId, record);
    return record;
  }

  private async run(record: DocumentDraftRecord, writers: DocumentDraftWriters): Promise<boolean> {
    while (this.isDirty(record)) {
      const targetGeneration = record.generation;
      const markdown = record.markdown;
      const title = record.title;
      const outcome = await flushDocument({
        md: markdown,
        title,
        lastSavedMd: record.savedMarkdown,
        savedTitle: record.savedTitle,
        writeContent: (value) => writers.writeContent(record.documentId, value),
        writeTitle: (value) => writers.writeTitle(record.documentId, value),
      });

      record.savedMarkdown = outcome.lastSavedMd;
      record.savedTitle = outcome.savedTitle;
      if (outcome.status === 'error') {
        record.error = outcome.error ?? '保存失败';
        return false;
      }

      record.error = null;
      record.persistedGeneration = targetGeneration;
      // 若保存期间又输入，generation 已推进，while 会立即保存最新合并快照。
    }
    return true;
  }

  private isDirty(record: DocumentDraftRecord): boolean {
    return (
      record.generation > record.persistedGeneration ||
      record.markdown !== record.savedMarkdown ||
      record.title !== record.savedTitle ||
      record.error != null
    );
  }

  private snapshotOf(record: DocumentDraftRecord): DocumentDraftSnapshot {
    return {
      documentId: record.documentId,
      markdown: record.markdown,
      title: record.title,
      dirty: this.isDirty(record),
      saving: record.inFlight != null,
      error: record.error,
      generation: record.generation,
      persistedGeneration: record.persistedGeneration,
    };
  }
}

export const documentDraftQueue = new DocumentDraftQueue();
