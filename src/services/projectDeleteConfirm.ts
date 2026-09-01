/** 项目移除只解除工作室关联，不涉及本地文件或代码。 */
export function formatProjectRemoveConfirm(projectName: string): {
  buttonLabel: string;
  warning: string;
} {
  return {
    buttonLabel: '确认移除项目？',
    warning: `将从工作室移除项目「${projectName}」；本地文件与代码不会改变。`,
  };
}
