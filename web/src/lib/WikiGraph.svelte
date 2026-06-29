<script lang="ts">
	import { getClient, type GraphData, type GraphNode } from './client.js';
	import ELK from 'elkjs/lib/elk.bundled.js';

	interface Props {
		onSelectNode?: (slug: string) => void;
		width?: number;
		height?: number;
	}

	let { onSelectNode, width = 900, height = 600 }: Props = $props();

	const client = getClient();
	const elk = new ELK();

	let layoutNodes: LayoutNode[] = $state([]);
	let layoutEdges: LayoutEdge[] = $state([]);
	let loading = $state(true);
	let viewBox = $state(`0 0 ${width} ${height}`);

	interface LayoutNode {
		id: string;
		slug: string;
		title: string;
		page_type: string;
		x: number;
		y: number;
		width: number;
		height: number;
	}

	interface LayoutEdge {
		id: string;
		points: { x: number; y: number }[];
	}

	const NODE_WIDTH = 160;
	const NODE_HEIGHT = 40;

	$effect(() => {
		client.getGraph().then(async (data) => {
			const layout = await computeLayout(data);
			layoutNodes = layout.nodes;
			layoutEdges = layout.edges;
			if (layout.width && layout.height) {
				viewBox = `0 0 ${layout.width + 40} ${layout.height + 40}`;
			}
			loading = false;
		}).catch(() => { loading = false; });
	});

	async function computeLayout(data: GraphData) {
		const elkGraph = {
			id: 'root',
			layoutOptions: {
				'elk.algorithm': 'layered',
				'elk.direction': 'DOWN',
				'elk.spacing.nodeNode': '30',
				'elk.layered.spacing.nodeNodeBetweenLayers': '60',
			},
			children: data.nodes.map((n) => ({
				id: n.id,
				width: NODE_WIDTH,
				height: NODE_HEIGHT,
				labels: [{ text: n.title }],
			})),
			edges: data.edges.map((e, i) => ({
				id: `e${i}`,
				sources: [e.source],
				targets: [e.target],
			})),
		};

		const result = await elk.layout(elkGraph);

		const nodes: LayoutNode[] = (result.children ?? []).map((child) => {
			const orig = data.nodes.find((n) => n.id === child.id)!;
			return {
				id: child.id,
				slug: orig.slug,
				title: orig.title,
				page_type: orig.page_type,
				x: child.x ?? 0,
				y: child.y ?? 0,
				width: child.width ?? NODE_WIDTH,
				height: child.height ?? NODE_HEIGHT,
			};
		});

		const edges: LayoutEdge[] = (result.edges ?? []).map((edge) => ({
			id: edge.id,
			points: edge.sections?.[0]?.bendPoints
				? [edge.sections[0].startPoint, ...edge.sections[0].bendPoints, edge.sections[0].endPoint]
				: edge.sections?.[0]
					? [edge.sections[0].startPoint, edge.sections[0].endPoint]
					: [],
		}));

		return { nodes, edges, width: result.width, height: result.height };
	}

	function nodeColor(pageType: string): string {
		const colors: Record<string, string> = {
			Index: 'var(--mp-graph-color-index, #6366f1)',
			Concept: 'var(--mp-graph-color-concept, #2563eb)',
			Entity: 'var(--mp-graph-color-entity, #059669)',
			Decision: 'var(--mp-graph-color-decision, #d97706)',
			Leaf: 'var(--mp-graph-color-leaf, #64748b)',
		};
		return colors[pageType] ?? colors.Leaf;
	}

	function edgePath(points: { x: number; y: number }[]): string {
		if (points.length === 0) return '';
		let d = `M ${points[0].x} ${points[0].y}`;
		for (let i = 1; i < points.length; i++) {
			d += ` L ${points[i].x} ${points[i].y}`;
		}
		return d;
	}
</script>

<div class="mp-graph">
	{#if loading}
		<div class="mp-graph__loading">Loading graph...</div>
	{:else}
		<svg {width} {height} viewBox={viewBox} class="mp-graph__svg">
			<defs>
				<marker id="mp-arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
					<polygon points="0 0, 8 3, 0 6" fill="var(--mp-color-muted, #6b7280)" />
				</marker>
			</defs>

			{#each layoutEdges as edge}
				<path
					d={edgePath(edge.points)}
					fill="none"
					stroke="var(--mp-color-muted, #6b7280)"
					stroke-width="1.5"
					marker-end="url(#mp-arrowhead)"
					opacity="0.6"
				/>
			{/each}

			{#each layoutNodes as node}
				<g
					transform="translate({node.x}, {node.y})"
					class="mp-graph__node"
					onclick={() => onSelectNode?.(node.slug)}
					role="button"
					tabindex="0"
					onkeydown={(e) => { if (e.key === 'Enter') onSelectNode?.(node.slug); }}
				>
					<rect
						width={node.width}
						height={node.height}
						rx="6"
						fill={nodeColor(node.page_type)}
						opacity="0.9"
					/>
					<text
						x={node.width / 2}
						y={node.height / 2}
						text-anchor="middle"
						dominant-baseline="central"
						fill="white"
						font-size="11"
						font-weight="600"
					>
						{node.title.length > 18 ? node.title.slice(0, 16) + '…' : node.title}
					</text>
				</g>
			{/each}
		</svg>
	{/if}
</div>

<style>
	.mp-graph {
		border: 1px solid var(--mp-color-border, #e5e5e5);
		border-radius: var(--mp-radius, 4px);
		overflow: auto;
		background: var(--mp-graph-bg, #fafafa);
	}
	.mp-graph__svg {
		display: block;
	}
	.mp-graph__node {
		cursor: pointer;
	}
	.mp-graph__node:hover rect {
		opacity: 1;
		filter: brightness(1.1);
	}
	.mp-graph__loading {
		padding: var(--mp-spacing-lg, 2rem);
		text-align: center;
		color: var(--mp-color-muted, #6b7280);
	}
</style>
