import { getContext, setContext } from 'svelte';

const CLIENT_KEY = Symbol('mind-palace-client');

export interface PageSummary {
	slug: string;
	title: string;
	summary: string;
	page_type: string;
}

export interface PageFull {
	slug: string;
	title: string;
	summary: string;
	page_type: string;
	sections: { heading: string; content: string }[];
	links: string[];
}

export interface GraphNode {
	id: string;
	slug: string;
	title: string;
	page_type: string;
}

export interface GraphEdge {
	source: string;
	target: string;
	kind: string;
}

export interface GraphData {
	nodes: GraphNode[];
	edges: GraphEdge[];
}

export interface SearchResult {
	slug: string;
	title: string;
	summary: string;
	score: number;
}

// --- Access control (Spec 5) ---

export type PrincipalType = 'user' | 'group';
export type Level = 'view' | 'edit';
export type BaseVisibility = 'public' | 'private';

export interface GrantView {
	principal_type: PrincipalType;
	principal: string;
	level: Level;
}

export interface AccessView {
	slug: string;
	owner: string | null;
	base_visibility: BaseVisibility;
	grants: GrantView[];
	/** Whether the current identity may change grants/owner (owner-only). */
	can_manage: boolean;
	/** Whether the current identity may edit the page content. */
	can_edit: boolean;
}

export interface GroupView {
	id: string;
	name: string;
	members: string[];
	managers: string[];
	/** Whether the current identity may mutate this group (manager/admin). */
	can_manage: boolean;
}

export interface TreeNode {
	slug: string;
	title: string;
	page_type: string;
	edge_kind: string;
}

/** A secret REFERENCE — name + opaque backend locator. NEVER a value. */
export interface SecretRefView {
	name: string;
	reference: string;
}

export interface AuditEvent {
	timestamp: string;
	slug: string;
	action: string;
	agent_id: string | null;
	summary: string | null;
}

export interface MindPalaceClient {
	listPages(pageType?: string): Promise<PageSummary[]>;
	getPage(slug: string): Promise<PageFull>;
	createPage(data: {
		title: string;
		slug: string;
		summary: string;
		sections: { heading: string; content: string }[];
		page_type: string;
		links?: string[];
		base_visibility?: BaseVisibility;
		parent?: string;
	}): Promise<PageFull>;
	updatePage(
		slug: string,
		data: { title?: string; summary?: string; sections?: { heading: string; content: string }[]; links?: string[] }
	): Promise<PageFull>;
	deletePage(slug: string): Promise<void>;
	getGraph(): Promise<GraphData>;
	search(query: string): Promise<SearchResult[]>;

	// Access + grants
	getAccess(slug: string): Promise<AccessView>;
	setAccess(slug: string, data: { base_visibility?: BaseVisibility; owner?: string | null }): Promise<void>;
	addGrant(slug: string, grant: { principal_type: PrincipalType; principal: string; level: Level }): Promise<void>;
	removeGrant(slug: string, grant: { principal_type: PrincipalType; principal: string }): Promise<void>;

	// Groups
	listGroups(): Promise<GroupView[]>;
	createGroup(data: { id: string; name: string }): Promise<GroupView>;
	getGroup(id: string): Promise<GroupView>;
	addGroupMember(id: string, email: string): Promise<GroupView>;
	removeGroupMember(id: string, email: string): Promise<GroupView>;
	addGroupManager(id: string, email: string): Promise<GroupView>;
	removeGroupManager(id: string, email: string): Promise<GroupView>;

	// Hierarchy
	setParent(slug: string, parent: string | null): Promise<void>;
	getTree(slug: string): Promise<TreeNode[]>;

	// Secret references (names + refs only, never values)
	listSecretRefs(slug: string): Promise<SecretRefView[]>;
	addSecretRef(slug: string, data: { name: string; reference: string }): Promise<void>;
	removeSecretRef(slug: string, name: string): Promise<void>;

	// Audit
	getAudit(filter?: { subject?: string; resource?: string }): Promise<AuditEvent[]>;
}

export function createClient(apiUrl: string, token?: string): MindPalaceClient {
	const headers = (): Record<string, string> => {
		const h: Record<string, string> = { 'Content-Type': 'application/json' };
		if (token) h['Authorization'] = `Bearer ${token}`;
		return h;
	};

	async function request<T>(path: string, options?: RequestInit): Promise<T> {
		const res = await fetch(`${apiUrl}${path}`, { headers: headers(), ...options });
		if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
		return res.json();
	}

	// For endpoints that return 204 No Content (all mutations that don't echo a body).
	async function requestVoid(path: string, options?: RequestInit): Promise<void> {
		const res = await fetch(`${apiUrl}${path}`, { headers: headers(), ...options });
		if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
	}

	return {
		listPages: (pageType) => request(`/api/pages${pageType ? `?type=${pageType}` : ''}`),
		getPage: (slug) => request(`/api/pages/${slug}`),
		createPage: (data) => request('/api/pages', { method: 'POST', body: JSON.stringify(data) }),
		updatePage: (slug, data) => request(`/api/pages/${slug}`, { method: 'PUT', body: JSON.stringify(data) }),
		deletePage: (slug) => requestVoid(`/api/pages/${slug}`, { method: 'DELETE' }),
		getGraph: () => request('/api/graph'),
		search: (query) => request(`/api/search?q=${encodeURIComponent(query)}`),

		getAccess: (slug) => request(`/api/pages/${slug}/access`),
		setAccess: (slug, data) =>
			requestVoid(`/api/pages/${slug}/access`, { method: 'PUT', body: JSON.stringify(data) }),
		addGrant: (slug, grant) =>
			requestVoid(`/api/pages/${slug}/grants`, { method: 'POST', body: JSON.stringify(grant) }),
		removeGrant: (slug, grant) =>
			requestVoid(`/api/pages/${slug}/grants`, { method: 'DELETE', body: JSON.stringify(grant) }),

		listGroups: () => request('/api/groups'),
		createGroup: (data) => request('/api/groups', { method: 'POST', body: JSON.stringify(data) }),
		getGroup: (id) => request(`/api/groups/${id}`),
		addGroupMember: (id, email) =>
			request(`/api/groups/${id}/members`, { method: 'POST', body: JSON.stringify({ email }) }),
		removeGroupMember: (id, email) =>
			request(`/api/groups/${id}/members`, { method: 'DELETE', body: JSON.stringify({ email }) }),
		addGroupManager: (id, email) =>
			request(`/api/groups/${id}/managers`, { method: 'POST', body: JSON.stringify({ email }) }),
		removeGroupManager: (id, email) =>
			request(`/api/groups/${id}/managers`, { method: 'DELETE', body: JSON.stringify({ email }) }),

		setParent: (slug, parent) =>
			requestVoid(`/api/pages/${slug}/parent`, { method: 'PUT', body: JSON.stringify({ parent }) }),
		getTree: (slug) => request(`/api/pages/${slug}/tree`),

		listSecretRefs: (slug) => request(`/api/pages/${slug}/secret-refs`),
		addSecretRef: (slug, data) =>
			requestVoid(`/api/pages/${slug}/secret-refs`, { method: 'POST', body: JSON.stringify(data) }),
		removeSecretRef: (slug, name) =>
			requestVoid(`/api/pages/${slug}/secret-refs`, { method: 'DELETE', body: JSON.stringify({ name }) }),

		getAudit: (filter) => {
			const params = new URLSearchParams();
			if (filter?.subject) params.set('subject', filter.subject);
			if (filter?.resource) params.set('resource', filter.resource);
			const qs = params.toString();
			return request(`/api/audit${qs ? `?${qs}` : ''}`);
		},
	};
}

export function setClient(client: MindPalaceClient) {
	setContext(CLIENT_KEY, client);
}

export function getClient(): MindPalaceClient {
	return getContext<MindPalaceClient>(CLIENT_KEY);
}
