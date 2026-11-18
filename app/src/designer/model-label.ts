// Display-only: saved source, catalog values and emitted IDs stay exact.
const modelNames: Record<string, string> = { 'gpt-5.6-terra': 'GPT-5.6 Terra' };
export const modelLabel = (id: string) => Object.hasOwn(modelNames, id) ? modelNames[id] : id;
