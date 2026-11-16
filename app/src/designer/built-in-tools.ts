import unsupportedTools from './unsupported-tools.json';

// Orka 3cb1f0df: task_controller.go approval validation and aitools/gateway_reply.go eligibility.
// Match effective names: Orka resolves built-ins before same-named Tool resources.
export const unsupportedLocalTool = (name: string) => unsupportedTools.includes(name.trim());
export const unsupportedToolReason = 'Approval tools need autonomous coordination, which Bridge does not support for local tests or Chat.';
export const unsupportedLocalToolReason = (name: string) => name.trim() === 'reply_in_conversation'
  ? 'Conversation replies need an authenticated gateway conversation, which local tests and Chat do not have.'
  : unsupportedToolReason;
export const unsupportedToolWarning = 'Remove all request_approval and reply_in_conversation tools before running local tests or Chat.';
export const unsupportedToolMessage = `${unsupportedToolWarning} No work was submitted.`;

// Creator-facing descriptions for common built-ins; unknown tools keep their supplied text.
export const builtInToolDescriptions: Readonly<Record<string, string>> = {
  web_fetch: 'Read the contents of a web page at an address.',
  web_search: 'Find current information on the web.',
  file_read: 'Read a file available to the agent.',
  file_write: 'Create or update a file available to the agent.',
  code_exec: 'Run code and read its output.',
  request_approval: 'Ask a person to approve an action.',
  delegate_task: 'Pass a task to another agent.',
  reply_in_conversation: 'Send a reply in the current conversation.',
};

// Orka KnownBuiltInToolNames union at image pin 3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea.
// This is a naming map, not a claim that a workspace has enabled each tool.
export const builtInTools: Readonly<Record<string, string>> = {
  web_fetch: 'Read a web page', web_search: 'Search the web',
  file_read: 'Read a file', file_write: 'Write a file', code_exec: 'Run code',
  request_approval: 'Ask for approval', reply_in_conversation: 'Reply in a conversation',
  delegate_task: 'Delegate a task',
  wait_for_tasks: 'Wait for tasks', create_container_task: 'Run a task in a container',
  cancel_task: 'Cancel a task', send_message: 'Send a message', check_messages: 'Read messages',
  create_pull_request: 'Open a pull request', check_pull_request_ci: 'Check pull request checks',
  merge_pull_request: 'Merge a pull request', auto_merge_pull_request: 'Set a pull request to merge',
  review_pull_request: 'Review a pull request', post_review_comment: 'Comment on a review',
  check_pr_review_marker: 'Check a review marker', list_issues: 'List issues',
  list_pull_requests: 'List pull requests', get_issue: 'Read an issue', comment_on_issue: 'Comment on an issue',
  create_agent: 'Create an agent', delete_agent: 'Delete an agent', update_agent: 'Change an agent',
  update_plan: 'Update a plan', recall_memory: 'Recall a memory', remember: 'Save a memory',
  propose_memory: 'Suggest a memory', search_transcript: 'Search past conversations',
  create_ai_task: 'Create an AI task', create_pr_monitor: 'Watch a pull request',
  create_agent_task: 'Give an agent a task', check_task_progress: 'Check task progress',
  fetch_task_output: 'Read a task’s output', wait_for_task: 'Wait for a task',
  list_agents: 'List agents', list_tools: 'List tools', list_tasks: 'List tasks',
  list_connections: 'List connected accounts',
  create_tool: 'Create a tool', delete_tool: 'Delete a tool', delete_session: 'Delete a conversation',
};
