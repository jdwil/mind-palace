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

export interface MindPalaceClient {
	listPages(pageType?: string): Promise<PageSummary[]>;
	getPage(slug: string): Promise<PageFull>;
	createPage(data: { title: string; slug: string; summary: string; sections: { heading: string; content: string }[]; page_type: string; links?: string[] }): Promise<PageFull>;
	updatePage(slug: string, data: { title?: string; summary?: string; sections?: { heading: string; content: string }[]; links?: string[] }): Promise<PageFull>;
	deletePage(slug: string): Promise<void>;
	getGraph(): Promise<GraphData>;
	search(query: string): Promise<SearchResult[]>;
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

	return {
		listPages: (pageType) => request(`/api/pages${pageType ? `?type=${pageType}` : ''}`),
		getPage: (slug) => request(`/api/pages/${slug}`),
		createPage: (data) => request('/api/pages', { method: 'POST', body: JSON.stringify(data) }),
		updatePage: (slug, data) => request(`/api/pages/${slug}`, { method: 'PUT', body: JSON.stringify(data) }),
		deletePage: (slug) => request(`/api/pages/${slug}`, { method: 'DELETE' }) as unknown as Promise<void>,
		getGraph: () => request('/api/graph'),
		search: (query) => request(`/api/search?q=${encodeURIComponent(query)}`),
	};
}

export function setClient(client: MindPalaceClient) {
	setContext(CLIENT_KEY, client);
}

export function getClient(): MindPalaceClient {
	return getContext<MindPalaceClient>(CLIENT_KEY);
}
