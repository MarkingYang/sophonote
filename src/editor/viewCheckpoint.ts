/** 编辑器应用 Agent patch 前的可恢复视图状态；正文快照由后端 revision 持久化。 */
export interface EditorViewCheckpoint {
  anchor: number;
  head: number;
  scrollTop: number;
  scrollLeft: number;
  focused: boolean;
}
