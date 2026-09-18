export { default as MindPalaceProvider } from './MindPalaceProvider.svelte';
export { default as WikiBrowser } from './WikiBrowser.svelte';
export { default as WikiPage } from './WikiPage.svelte';
export { default as WikiEditor } from './WikiEditor.svelte';
export { default as WikiGraph } from './WikiGraph.svelte';
export { default as WikiSearch } from './WikiSearch.svelte';

// Spec 5 — management components
export { default as AccessPanel } from './AccessPanel.svelte';
export { default as GroupManager } from './GroupManager.svelte';
export { default as HierarchyView } from './HierarchyView.svelte';
export { default as SecretRefs } from './SecretRefs.svelte';

export {
	getClient,
	setClient,
	createClient,
	type MindPalaceClient,
	type PageSummary,
	type PageFull,
	type GraphData,
	type GraphNode,
	type GraphEdge,
	type SearchResult,
	// Access-control types (Spec 5)
	type PrincipalType,
	type Level,
	type BaseVisibility,
	type GrantView,
	type AccessView,
	type GroupView,
	type TreeNode,
	type SecretRefView,
	type AuditEvent
} from './client.js';
