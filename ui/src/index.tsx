import { render } from "solid-js/web";
import { createSignal, onMount, onCleanup, For, Show, createEffect } from "solid-js";
import "./index.css";

interface Endpoint {
  method: string;
  path: string;
}

interface SpecInfo {
  title: string;
  version: string;
}

interface TableInfo {
  name: string;
  row_count: number;
}

interface ColumnInfo {
  name: string;
  type: string;
}

interface TableData {
  columns: ColumnInfo[];
  rows: Record<string, unknown>[];
}

interface LogEntry {
  timestamp: string;
  method: string;
  path: string;
  status: number;
}

interface Recipe {
  id: number;
  name: string;
  spec_source: string;
  selected_endpoints: string;
  seed_count: number;
  created_at: string;
  shared_pools: string;
  quantity_configs: string;
}

interface PropertyInfo {
  type: string;
  format: string | null;
  required: boolean;
  ref_name: string | null;
  is_array: boolean;
  items_ref: string | null;
  enum_values: string[] | null;
  description: string | null;
}

interface DefinitionInfo {
  description?: string;
  extends?: string;
  properties: Record<string, PropertyInfo>;
}

interface RouteInfo {
  method: string;
  path: string;
  definition: string;
}

type Page = "dashboard" | "endpoints" | "tables" | "schemas" | "log" | "recipes";
type WizardState = "idle" | "selecting" | "running";

function App() {
  const [page, setPage] = createSignal<Page>("dashboard");
  const [state, setState] = createSignal<WizardState>("idle");
  const [specInfo, setSpecInfo] = createSignal<SpecInfo | null>(null);
  const [availableEndpoints, setAvailableEndpoints] = createSignal<Endpoint[]>([]);
  const [selected, setSelected] = createSignal<boolean[]>([]);
  const [activeEndpoints, setActiveEndpoints] = createSignal<Endpoint[]>([]);
  const [seedCount, setSeedCount] = createSignal(10);
  const [error, setError] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);

  const [tables, setTables] = createSignal<TableInfo[]>([]);
  const [selectedTable, setSelectedTable] = createSignal<string | null>(null);
  const [tableData, setTableData] = createSignal<TableData | null>(null);
  const [tableLoading, setTableLoading] = createSignal(false);

  // Log state
  const [logEntries, setLogEntries] = createSignal<LogEntry[]>([]);

  // Recipe state
  const [recipes, setRecipes] = createSignal<Recipe[]>([]);
  const [recipeCreating, setRecipeCreating] = createSignal(false);
  const [recipeSpecText, setRecipeSpecText] = createSignal("");
  const [recipeName, setRecipeName] = createSignal("");
  const [recipeSeedCount, setRecipeSeedCount] = createSignal(10);
  const [recipeAvailableEndpoints, setRecipeAvailableEndpoints] = createSignal<Endpoint[]>([]);
  const [recipeSelectedEndpoints, setRecipeSelectedEndpoints] = createSignal<boolean[]>([]);
  const [recipeStep, setRecipeStep] = createSignal<"paste" | "select" | "graph" | "config" | "name">("paste");
  const [entityGraph, setEntityGraph] = createSignal<any>(null);
  const [graphLoading, setGraphLoading] = createSignal(false);
  const [recipeSharedPools, setRecipeSharedPools] = createSignal<Record<string, {is_shared: boolean, pool_size: number}>>({});
  const [recipeQuantityConfigs, setRecipeQuantityConfigs] = createSignal<Record<string, {min: number, max: number}>>({});
  const [editingRecipeId, setEditingRecipeId] = createSignal<number | null>(null);

  // Schema state
  const [definitions, setDefinitions] = createSignal<Record<string, DefinitionInfo>>({});
  const [schemaRoutes, setSchemaRoutes] = createSignal<RouteInfo[]>([]);
  const [expandedDefs, setExpandedDefs] = createSignal<Set<string>>(new Set());
  const [selectedDef, setSelectedDef] = createSignal<string | null>(null);

  onMount(async () => {
    try {
      const specRes = await fetch("/_api/admin/spec");
      const spec: SpecInfo = await specRes.json();
      if (spec.version !== "No spec loaded") {
        setSpecInfo(spec);
        const epRes = await fetch("/_api/admin/endpoints");
        const eps: Endpoint[] = await epRes.json();
        if (eps.length > 0) {
          setActiveEndpoints(eps);
          setState("running");
        }
      }
    } catch {
      // Stay in idle state
    }
  });

  const refreshTables = async () => {
    try {
      const res = await fetch("/_api/admin/tables");
      const data: TableInfo[] = await res.json();
      setTables(data);
    } catch {
      // ignore
    }
  };

  createEffect(() => {
    if (page() === "tables" || page() === "dashboard") {
      refreshTables();
    }
  });

  const refreshLog = async () => {
    try {
      const res = await fetch("/_api/admin/log");
      const data: LogEntry[] = await res.json();
      setLogEntries(data);
    } catch {
      // ignore
    }
  };

  const refreshRecipes = async () => {
    try {
      const res = await fetch("/_api/admin/recipes");
      const data: Recipe[] = await res.json();
      setRecipes(data);
    } catch {
      // ignore
    }
  };

  createEffect(() => {
    if (page() === "recipes") {
      refreshRecipes();
    }
  });

  createEffect(() => {
    if (page() === "schemas") {
      (async () => {
        try {
          const [defRes, routesRes] = await Promise.all([
            fetch("/_api/admin/definitions"),
            fetch("/_api/admin/routes"),
          ]);
          setDefinitions(await defRes.json());
          setSchemaRoutes(await routesRes.json());
        } catch {
          // ignore
        }
      })();
    }
  });

  // Poll log every 2s when on the log page
  createEffect(() => {
    if (page() === "log") {
      refreshLog();
      const id = setInterval(refreshLog, 2000);
      onCleanup(() => clearInterval(id));
    }
  });

  const loadTableData = async (name: string) => {
    setSelectedTable(name);
    setTableLoading(true);
    try {
      const res = await fetch(`/_api/admin/tables/${encodeURIComponent(name)}`);
      const data: TableData = await res.json();
      setTableData(data);
    } catch {
      setTableData(null);
    }
    setTableLoading(false);
  };

  const handleImport = async () => {
    const textarea = document.getElementById("spec-input") as HTMLTextAreaElement;
    const value = textarea?.value?.trim();
    if (!value) {
      setError("Please paste a spec first.");
      return;
    }
    setError(null);
    setLoading(true);
    try {
      const res = await fetch("/_api/admin/import", {
        method: "POST",
        headers: { "Content-Type": "text/plain" },
        body: value,
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        return;
      }
      const data = await res.json();
      setSpecInfo(data.spec_info);
      setAvailableEndpoints(data.endpoints);
      setSelected(data.endpoints.map(() => true));
      setSeedCount(10);
      setState("selecting");
      setPage("endpoints");
    } catch (e: any) {
      setError(String(e?.message || e));
    }
    setLoading(false);
  };

  const toggleEndpoint = (index: number) => {
    setSelected((prev) => {
      const next = [...prev];
      next[index] = !next[index];
      return next;
    });
  };

  const handleConfigure = async () => {
    const endpoints = availableEndpoints().filter((_, i) => selected()[i]);
    if (endpoints.length === 0) {
      setError("Select at least one endpoint.");
      return;
    }
    setError(null);
    setLoading(true);
    try {
      const res = await fetch("/_api/admin/configure", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ endpoints, seed_count: seedCount() }),
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        return;
      }
      const epRes = await fetch("/_api/admin/endpoints");
      const activeEps: Endpoint[] = await epRes.json();
      setActiveEndpoints(activeEps);
      setState("running");
    } catch (e: any) {
      setError(String(e?.message || e));
    }
    setLoading(false);
  };

  const handleRecipeParseSpec = async () => {
    const value = recipeSpecText().trim();
    if (!value) {
      setError("Please paste a spec first.");
      return;
    }
    setError(null);
    setLoading(true);
    try {
      const res = await fetch("/_api/admin/import", {
        method: "POST",
        headers: { "Content-Type": "text/plain" },
        body: value,
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        return;
      }
      const data = await res.json();
      setRecipeAvailableEndpoints(data.endpoints);
      setRecipeSelectedEndpoints(data.endpoints.map(() => true));
      setRecipeStep("select");
    } catch (e: any) {
      setError(String(e?.message || e));
    }
    setLoading(false);
  };

  const toggleRecipeEndpoint = (index: number) => {
    setRecipeSelectedEndpoints((prev) => {
      const next = [...prev];
      next[index] = !next[index];
      return next;
    });
  };

  const handleFetchGraph = async () => {
    setGraphLoading(true);
    setError(null);
    try {
      const resp = await fetch("/_api/admin/graph", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          spec_source: recipeSpecText(),
          endpoints: recipeAvailableEndpoints()
            .filter((_, i) => recipeSelectedEndpoints()[i])
            .map((e) => ({ method: e.method, path: e.path })),
        }),
      });
      if (!resp.ok) throw new Error("Failed to compute graph");
      setEntityGraph(await resp.json());
      setRecipeStep("graph");
    } catch (e: any) {
      setError(String(e?.message || e));
    } finally {
      setGraphLoading(false);
    }
  };

  const handleGoToConfig = () => {
    const graph = entityGraph();
    if (!graph) return;

    const pools: Record<string, {is_shared: boolean, pool_size: number}> = {};
    for (const entity of graph.shared_entities || []) {
      pools[entity] = { is_shared: true, pool_size: 10 };
    }
    setRecipeSharedPools(pools);

    const configs: Record<string, {min: number, max: number}> = {};
    for (const ap of graph.array_properties || []) {
      configs[`${ap.def_name}.${ap.prop_name}`] = { min: 1, max: 3 };
    }
    setRecipeQuantityConfigs(configs);

    setRecipeStep("config");
  };

  const [saveActivatePhase, setSaveActivatePhase] = createSignal<"idle" | "saving" | "activating">("idle");
  const [savedRecipeId, setSavedRecipeId] = createSignal<number | null>(null);

  const handleRecipeSaveAndActivate = async () => {
    const name = recipeName().trim();
    if (!name) {
      setError("Please enter a recipe name.");
      return;
    }
    const endpoints = recipeAvailableEndpoints().filter((_, i) => recipeSelectedEndpoints()[i]);
    if (endpoints.length === 0) {
      setError("Select at least one endpoint.");
      return;
    }
    setError(null);
    setLoading(true);

    const editId = editingRecipeId();
    if (editId !== null) {
      // Edit mode: PUT to update, no activation
      setSaveActivatePhase("saving");
      try {
        const res = await fetch(`/_api/admin/recipes/${editId}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            name,
            spec_source: recipeSpecText().trim(),
            endpoints,
            seed_count: recipeSeedCount(),
            shared_pools: recipeSharedPools(),
            quantity_configs: recipeQuantityConfigs(),
          }),
        });
        if (!res.ok) {
          const text = await res.text();
          try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
          setLoading(false);
          setSaveActivatePhase("idle");
          return;
        }
        // Reset wizard state
        setRecipeCreating(false);
        setRecipeSpecText("");
        setRecipeName("");
        setRecipeSeedCount(10);
        setRecipeAvailableEndpoints([]);
        setRecipeSelectedEndpoints([]);
        setRecipeStep("paste");
        setEntityGraph(null);
        setRecipeSharedPools({});
        setRecipeQuantityConfigs({});
        setEditingRecipeId(null);
        setSaveActivatePhase("idle");
        await refreshRecipes();
      } catch (e: any) {
        setError(String(e?.message || e));
        setSaveActivatePhase("idle");
      }
      setLoading(false);
      return;
    }

    // Create mode: POST then activate
    setSaveActivatePhase("saving");
    setSavedRecipeId(null);

    // Phase 1: Save
    let recipeId: number;
    try {
      const res = await fetch("/_api/admin/recipes", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          name,
          spec_source: recipeSpecText().trim(),
          endpoints,
          seed_count: recipeSeedCount(),
          shared_pools: recipeSharedPools(),
          quantity_configs: recipeQuantityConfigs(),
        }),
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        setSaveActivatePhase("idle");
        return;
      }
      const saved = await res.json();
      recipeId = saved.id;
      setSavedRecipeId(recipeId);
    } catch (e: any) {
      setError(String(e?.message || e));
      setLoading(false);
      setSaveActivatePhase("idle");
      return;
    }

    // Phase 2: Activate
    await activateSavedRecipe(recipeId);
  };

  const activateSavedRecipe = async (id: number) => {
    setError(null);
    setLoading(true);
    setSaveActivatePhase("activating");
    try {
      const res = await fetch(`/_api/admin/recipes/${id}/activate`, {
        method: "POST",
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError("Recipe saved but activation failed: " + (JSON.parse(text).error || text)); } catch { setError(`Recipe saved but activation failed: ${res.status}: ${text}`); }
        setLoading(false);
        setSaveActivatePhase("idle");
        await refreshRecipes();
        return;
      }
      const data = await res.json();
      setActiveEndpoints(data.endpoints);
      const specRes = await fetch("/_api/admin/spec");
      const spec: SpecInfo = await specRes.json();
      setSpecInfo(spec);
      setState("running");
      // Reset creation state
      setRecipeCreating(false);
      setRecipeSpecText("");
      setRecipeName("");
      setRecipeSeedCount(10);
      setRecipeAvailableEndpoints([]);
      setRecipeSelectedEndpoints([]);
      setRecipeStep("paste");
      setRecipeSharedPools({});
      setRecipeQuantityConfigs({});
      setEntityGraph(null);
      setSavedRecipeId(null);
      setSaveActivatePhase("idle");
      setPage("dashboard");
    } catch (e: any) {
      setError("Recipe saved but activation failed: " + String(e?.message || e));
      setSaveActivatePhase("idle");
      await refreshRecipes();
    }
    setLoading(false);
  };

  const handleRecipeActivate = async (id: number) => {
    setError(null);
    setLoading(true);
    try {
      const res = await fetch(`/_api/admin/recipes/${id}/activate`, {
        method: "POST",
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        return;
      }
      const data = await res.json();
      setActiveEndpoints(data.endpoints);
      // Refresh spec info
      const specRes = await fetch("/_api/admin/spec");
      const spec: SpecInfo = await specRes.json();
      setSpecInfo(spec);
      setState("running");
    } catch (e: any) {
      setError(String(e?.message || e));
    }
    setLoading(false);
  };

  const handleRecipeDelete = async (id: number) => {
    setError(null);
    try {
      const res = await fetch(`/_api/admin/recipes/${id}`, {
        method: "DELETE",
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        return;
      }
      await refreshRecipes();
    } catch (e: any) {
      setError(String(e?.message || e));
    }
  };

  const handleRecipeEdit = async (recipe: Recipe) => {
    setError(null);
    setLoading(true);
    try {
      // Import the spec to get available endpoints
      const res = await fetch("/_api/admin/import", {
        method: "POST",
        headers: { "Content-Type": "text/plain" },
        body: recipe.spec_source,
      });
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        setLoading(false);
        return;
      }
      const data = await res.json();
      const availableEps: Endpoint[] = data.endpoints;

      // Parse selected endpoints from recipe
      let selectedEps: Endpoint[] = [];
      try { selectedEps = JSON.parse(recipe.selected_endpoints); } catch { /* empty */ }

      // Map to boolean array: true if the available endpoint is in the selected list
      const selectedFlags = availableEps.map((ep) =>
        selectedEps.some((sel) => sel.method === ep.method && sel.path === ep.path)
      );

      // Parse shared_pools and quantity_configs
      let sharedPools: Record<string, {is_shared: boolean, pool_size: number}> = {};
      try { sharedPools = JSON.parse(recipe.shared_pools); } catch { /* empty */ }
      let quantityConfigs: Record<string, {min: number, max: number}> = {};
      try { quantityConfigs = JSON.parse(recipe.quantity_configs); } catch { /* empty */ }

      setRecipeSpecText(recipe.spec_source);
      setRecipeAvailableEndpoints(availableEps);
      setRecipeSelectedEndpoints(selectedFlags);
      setRecipeName(recipe.name);
      setRecipeSeedCount(recipe.seed_count);
      setRecipeSharedPools(sharedPools);
      setRecipeQuantityConfigs(quantityConfigs);
      setEditingRecipeId(recipe.id);
      setRecipeStep("select");
      setRecipeCreating(true);
    } catch (e: any) {
      setError(String(e?.message || e));
    }
    setLoading(false);
  };

  const handleRecipeCancelCreate = () => {
    setRecipeCreating(false);
    setRecipeSpecText("");
    setRecipeName("");
    setRecipeSeedCount(10);
    setRecipeAvailableEndpoints([]);
    setRecipeSelectedEndpoints([]);
    setRecipeStep("paste");
    setEntityGraph(null);
    setRecipeSharedPools({});
    setRecipeQuantityConfigs({});
    setEditingRecipeId(null);
    setError(null);
  };

  const toggleDef = (name: string) => {
    const next = new Set(expandedDefs());
    if (next.has(name)) next.delete(name);
    else next.add(name);
    setExpandedDefs(next);
  };

  const selectDef = (name: string) => {
    setSelectedDef(name);
    if (!expandedDefs().has(name)) toggleDef(name);
  };

  const endpointsForDef = (defName: string) =>
    schemaRoutes().filter(r => r.definition === defName);

  const typeBadgeClass = (type: string, isRef: boolean, isEnum: boolean) => {
    if (isEnum) return "bg-pink-500/10 text-pink-400";
    if (isRef) return "bg-purple-500/10 text-purple-400";
    switch (type) {
      case "string": return "bg-green-500/10 text-green-400";
      case "integer": case "number": return "bg-blue-500/10 text-blue-400";
      case "boolean": return "bg-yellow-500/10 text-yellow-400";
      case "array": return "bg-orange-500/10 text-orange-400";
      default: return "bg-purple-500/10 text-purple-400";
    }
  };

  const navItems: { id: Page; label: string; icon: string }[] = [
    { id: "dashboard", label: "Dashboard", icon: "M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" },
    { id: "endpoints", label: "Endpoints", icon: "M13 10V3L4 14h7v7l9-11h-7z" },
    { id: "tables", label: "Tables", icon: "M3 10h18M3 14h18m-9-4v8m-7 0h14a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" },
    { id: "schemas", label: "Schemas", icon: "M20 7l-8-4-8 4m16 0l-8 4m8-4v10l-8 4m0-10L4 7m8 4v10M4 7v10l8 4" },
    { id: "recipes", label: "Recipes", icon: "M12 6.042A8.967 8.967 0 006 3.75c-1.052 0-2.062.18-3 .512v14.25A8.987 8.987 0 016 18c2.305 0 4.408.867 6 2.292m0-14.25a8.966 8.966 0 016-2.292c1.052 0 2.062.18 3 .512v14.25A8.987 8.987 0 0018 18a8.967 8.967 0 00-6 2.292m0-14.25v14.25" },
    { id: "log", label: "Log", icon: "M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2" },
  ];

  return (
    <div class="min-h-screen text-gray-100 flex">
      {/* Sidebar */}
      <nav class="w-52 shrink-0 bg-gray-900 border-r border-gray-800 flex flex-col sticky top-0 h-screen">
        <div class="p-5 pb-6">
          <h1 class="text-lg font-semibold tracking-tight">Mirage</h1>
          <Show when={specInfo() && state() === "running"}>
            <p class="text-xs text-gray-500 mt-0.5">{specInfo()?.title} v{specInfo()?.version}</p>
          </Show>
        </div>

        <div class="flex-1 px-3 space-y-0.5">
          <For each={navItems}>
            {(item) => (
              <button
                class={`w-full flex items-center gap-2.5 px-3 py-2 rounded-md text-sm transition-all ${
                  page() === item.id
                    ? "bg-blue-600/15 text-blue-400 font-medium"
                    : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                }`}
                onClick={() => setPage(item.id)}
              >
                <svg class="w-4 h-4 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
                  <path stroke-linecap="round" stroke-linejoin="round" d={item.icon} />
                </svg>
                {item.label}
              </button>
            )}
          </For>
        </div>

        <Show when={state() === "running"}>
          <div class="p-4 mx-3 mb-3 rounded-md bg-green-500/10 border border-green-500/20">
            <div class="flex items-center gap-2">
              <div class="w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
              <span class="text-xs text-green-400 font-medium">Server running</span>
            </div>
          </div>
        </Show>
      </nav>

      {/* Main */}
      <main class="flex-1 min-h-screen">
        <div class={`p-8 ${page() === "schemas" ? "max-w-7xl" : "max-w-5xl"}`}>
          <Show when={error()}>
            <div class="mb-6 px-4 py-3 rounded-md bg-red-500/10 border border-red-500/20 text-red-400 text-sm">
              {error()}
            </div>
          </Show>

          {/* === Dashboard === */}
          <Show when={page() === "dashboard"}>
            <Show when={state() === "idle"}>
              <h2 class="text-2xl font-semibold mb-1">Dashboard</h2>
              <p class="text-gray-500 mb-8">Paste a Swagger 2.0 spec to get started.</p>
              <textarea
                id="spec-input"
                class="w-full h-64 bg-[#070c17] border border-gray-800 rounded-lg p-4 font-mono text-sm text-gray-300 resize-y placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700 transition-colors"
                placeholder="Paste Swagger 2.0 YAML or JSON here..."
              />
              <button
                id="import-btn"
                class="mt-4 px-5 py-2.5 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
                onClick={handleImport}
                disabled={loading()}
              >
                {loading() ? "Importing..." : "Import Spec"}
              </button>
            </Show>

            <Show when={state() !== "idle"}>
              <h2 class="text-2xl font-semibold mb-8">Dashboard</h2>
              <div class="grid grid-cols-3 gap-5">
                <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-5">
                  <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Spec</p>
                  <p class="text-lg font-semibold truncate">{specInfo()?.title}</p>
                  <p class="text-sm text-gray-500 mt-0.5">v{specInfo()?.version}</p>
                </div>
                <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-5">
                  <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Endpoints</p>
                  <p class="text-3xl font-bold tabular-nums">{activeEndpoints().length}</p>
                  <p class="text-sm text-gray-500 mt-0.5">active routes</p>
                </div>
                <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-5">
                  <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Tables</p>
                  <p class="text-3xl font-bold tabular-nums">{tables().length}</p>
                  <p class="text-sm text-gray-500 mt-0.5">{tables().reduce((s, t) => s + t.row_count, 0)} rows</p>
                </div>
              </div>
            </Show>
          </Show>

          {/* === Endpoints === */}
          <Show when={page() === "endpoints"}>
            <Show when={state() === "idle"}>
              <h2 class="text-2xl font-semibold mb-4">Endpoints</h2>
              <p class="text-gray-500">Import a spec from the Dashboard to configure endpoints.</p>
            </Show>

            <Show when={state() === "selecting"}>
              <div class="flex items-center justify-between mb-6">
                <h2 class="text-2xl font-semibold">Select Endpoints</h2>
                <div class="flex gap-2">
                  <button
                    class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                    onClick={() => setSelected(availableEndpoints().map(() => true))}
                  >
                    Select All
                  </button>
                  <button
                    class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                    onClick={() => setSelected(availableEndpoints().map(() => false))}
                  >
                    Deselect All
                  </button>
                </div>
              </div>
              <div id="endpoint-list" class="space-y-0.5 mb-6">
                <For each={availableEndpoints()}>
                  {(ep, i) => (
                    <label class="flex items-center gap-3 px-3 py-2.5 rounded-md cursor-pointer hover:bg-white/[0.03] transition-colors">
                      <input
                        type="checkbox"
                        checked={selected()[i()]}
                        onChange={() => toggleEndpoint(i())}
                        class="endpoint-checkbox accent-blue-500 rounded"
                      />
                      <MethodBadge method={ep.method} />
                      <span class="font-mono text-sm text-gray-300">{ep.path}</span>
                    </label>
                  )}
                </For>
              </div>
              <div class="flex items-center gap-4 pt-4 border-t border-gray-800/60">
                <label class="flex items-center gap-2 text-sm text-gray-400">
                  Seed rows
                  <input
                    id="seed-count"
                    type="number"
                    value={seedCount()}
                    min={1}
                    max={100}
                    onInput={(e) => setSeedCount(parseInt(e.currentTarget.value) || 1)}
                    class="w-16 bg-[#070c17] border border-gray-800 rounded-md px-2.5 py-1.5 text-sm text-gray-100 focus:outline-none focus:border-gray-700"
                  />
                </label>
                <button
                  id="start-btn"
                  class="px-5 py-2 bg-green-600 hover:bg-green-500 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
                  onClick={handleConfigure}
                  disabled={loading()}
                >
                  {loading() ? "Configuring..." : "Start Mock Server"}
                </button>
              </div>
            </Show>

            <Show when={state() === "running"}>
              <h2 class="text-2xl font-semibold mb-6">Active Endpoints</h2>
              <div class="rounded-xl border border-[#141b28] overflow-hidden">
                <table class="w-full text-left">
                  <thead>
                    <tr class="bg-[#090e1a]">
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider w-28">Method</th>
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider">Path</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={activeEndpoints()}>
                      {(ep) => (
                        <tr class="border-t border-[#0e1521] hover:bg-white/[0.02] transition-colors">
                          <td class="py-2.5 px-4">
                            <MethodBadge method={ep.method} />
                          </td>
                          <td class="py-2.5 px-4 font-mono text-sm text-gray-300">{ep.path}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>
          </Show>

          {/* === Tables === */}
          <Show when={page() === "tables"}>
            <Show when={state() === "idle"}>
              <h2 class="text-2xl font-semibold mb-4">Tables</h2>
              <p class="text-gray-500">Import a spec from the Dashboard to browse tables.</p>
            </Show>

            <Show when={state() !== "idle"}>
              <h2 class="text-2xl font-semibold mb-6">Tables</h2>
              <Show when={tables().length === 0}>
                <p class="text-gray-500">No tables yet. Configure endpoints to generate tables.</p>
              </Show>

              <Show when={tables().length > 0}>
                <div class="flex gap-6">
                  {/* Table list */}
                  <div class="w-44 shrink-0 space-y-0.5">
                    <For each={tables()}>
                      {(t) => (
                        <button
                          class={`w-full flex items-center justify-between px-3 py-2 rounded-md text-sm transition-all ${
                            selectedTable() === t.name
                              ? "text-white font-medium"
                              : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                          }`}
                          onClick={() => loadTableData(t.name)}
                        >
                          <span>{t.name}</span>
                          <span class="text-xs tabular-nums opacity-50">{t.row_count}</span>
                        </button>
                      )}
                    </For>
                  </div>

                  {/* Table data */}
                  <div class="flex-1 min-w-0">
                    <Show when={!selectedTable() && !tableLoading()}>
                      <p class="text-gray-600 text-sm py-8 text-center">Select a table to view its data.</p>
                    </Show>

                    <Show when={tableLoading()}>
                      <p class="text-gray-500 text-sm py-8 text-center">Loading...</p>
                    </Show>

                    <Show when={selectedTable() && !tableLoading() && tableData()}>
                      <div class="rounded-xl border border-[#141b28] overflow-hidden">
                        <div class="overflow-x-auto">
                          <table class="w-full text-left">
                            <thead>
                              <tr class="bg-[#090e1a]">
                                <For each={tableData()!.columns}>
                                  {(col) => (
                                    <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider whitespace-nowrap">
                                      {col.name}
                                    </th>
                                  )}
                                </For>
                              </tr>
                            </thead>
                            <tbody>
                              <For each={tableData()!.rows}>
                                {(row) => (
                                  <tr class="border-t border-[#0e1521] hover:bg-white/[0.02] transition-colors">
                                    <For each={tableData()!.columns}>
                                      {(col) => (
                                        <td class="py-2.5 px-4 font-mono text-xs text-gray-300 whitespace-nowrap max-w-[200px] truncate" title={rawValue(row[col.name])}>
                                          {formatCell(row[col.name])}
                                        </td>
                                      )}
                                    </For>
                                  </tr>
                                )}
                              </For>
                            </tbody>
                          </table>
                        </div>
                      </div>
                    </Show>
                  </div>
                </div>
              </Show>
            </Show>
          </Show>

          {/* === Schemas === */}
          <Show when={page() === "schemas"}>
            <h2 class="text-2xl font-semibold mb-6">Schemas</h2>
            <Show when={Object.keys(definitions()).length === 0}>
              <p class="text-gray-500">No definitions available. Import a spec first.</p>
            </Show>
            <Show when={Object.keys(definitions()).length > 0}>
              <div class="flex gap-6" style="max-width: 100%; min-height: 400px;">
                {/* Left panel - Definition list */}
                <div class="w-72 shrink-0">
                  <div class="rounded-xl bg-[#0a101d] border border-[#141b28] overflow-hidden">
                    <div class="px-4 py-3 border-b border-[#141b28] flex items-center justify-between">
                      <span class="text-xs font-medium text-gray-500 uppercase tracking-wider">Definitions</span>
                      <span class="text-xs text-gray-600 tabular-nums">{Object.keys(definitions()).length}</span>
                    </div>
                    <div class="max-h-[calc(100vh-200px)] overflow-y-auto">
                      <For each={Object.keys(definitions()).sort()}>
                        {(defName) => {
                          const def = () => definitions()[defName];
                          const epCount = () => endpointsForDef(defName).length;
                          const isSelected = () => selectedDef() === defName;
                          const isExpanded = () => expandedDefs().has(defName);
                          return (
                            <div>
                              <button
                                class={`w-full flex items-center gap-2 px-3 py-2 text-sm transition-all ${
                                  isSelected()
                                    ? "bg-blue-600/15 text-blue-400 font-medium"
                                    : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                                }`}
                                onClick={() => selectDef(defName)}
                              >
                                <svg
                                  class={`w-3 h-3 shrink-0 transition-transform ${isExpanded() ? "rotate-90" : ""}`}
                                  fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"
                                  onClick={(e: MouseEvent) => { e.stopPropagation(); toggleDef(defName); }}
                                >
                                  <path stroke-linecap="round" stroke-linejoin="round" d="M9 5l7 7-7 7" />
                                </svg>
                                <span class="truncate flex-1 text-left">{defName}</span>
                                <Show when={epCount() > 0}>
                                  <span class="text-[10px] tabular-nums px-1.5 py-0.5 rounded-full bg-blue-500/10 text-blue-400">{epCount()}</span>
                                </Show>
                              </button>
                              <Show when={def()?.extends}>
                                <div class="px-3 pl-8 pb-1">
                                  <span class="text-xs text-gray-600">extends </span>
                                  <span
                                    class="text-xs text-purple-400 cursor-pointer hover:underline"
                                    onClick={() => selectDef(def()!.extends!)}
                                  >{def()!.extends}</span>
                                </div>
                              </Show>
                              <Show when={isExpanded()}>
                                <div class="pb-1">
                                  <For each={Object.entries(def()?.properties || {})}>
                                    {([propName, prop]) => (
                                      <div class="flex items-center gap-1.5 px-3 pl-8 py-1 text-xs">
                                        <span class="text-gray-400 truncate">
                                          {propName}
                                          <Show when={prop.required}><span class="text-red-400">*</span></Show>
                                        </span>
                                        <Show when={prop.ref_name}>
                                          <span
                                            class="px-1.5 py-0.5 rounded text-xs font-mono bg-purple-500/10 text-purple-400 cursor-pointer hover:underline"
                                            onClick={() => selectDef(prop.ref_name!)}
                                          >{prop.ref_name}</span>
                                        </Show>
                                        <Show when={prop.is_array && prop.items_ref}>
                                          <span
                                            class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400 cursor-pointer hover:underline"
                                            onClick={() => selectDef(prop.items_ref!)}
                                          >[{prop.items_ref}]</span>
                                        </Show>
                                        <Show when={prop.is_array && !prop.items_ref}>
                                          <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400">[{prop.type}]</span>
                                        </Show>
                                        <Show when={prop.enum_values}>
                                          <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-pink-500/10 text-pink-400">enum</span>
                                        </Show>
                                        <Show when={!prop.ref_name && !prop.is_array && !prop.enum_values}>
                                          <span class={`px-1.5 py-0.5 rounded text-xs font-mono ${typeBadgeClass(prop.type, false, false)}`}>{prop.type}</span>
                                        </Show>
                                      </div>
                                    )}
                                  </For>
                                </div>
                              </Show>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  </div>
                </div>

                {/* Right panel - Detail view */}
                <div class="flex-1 min-w-0">
                  <Show when={!selectedDef()}>
                    <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-8 text-center">
                      <p class="text-gray-600 text-sm">Select a definition to view its details.</p>
                    </div>
                  </Show>
                  <Show when={selectedDef() && definitions()[selectedDef()!]}>
                    {(() => {
                      const defName = () => selectedDef()!;
                      const def = () => definitions()[defName()];
                      const eps = () => endpointsForDef(defName());
                      return (
                        <div class="rounded-xl bg-[#0a101d] border border-[#141b28] overflow-hidden">
                          <div class="px-6 py-5 border-b border-[#141b28]">
                            <h3 class="text-xl font-semibold text-gray-100">{defName()}</h3>
                            <Show when={def()?.description}>
                              <p class="text-sm text-gray-500 mt-1">{def()!.description}</p>
                            </Show>
                            <Show when={def()?.extends}>
                              <p class="text-sm text-gray-500 mt-1">
                                Extends:{" "}
                                <span
                                  class="text-purple-400 cursor-pointer hover:underline"
                                  onClick={() => selectDef(def()!.extends!)}
                                >{def()!.extends}</span>
                              </p>
                            </Show>
                          </div>

                          {/* Properties table */}
                          <div class="px-6 py-4">
                            <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Properties</p>
                            <Show when={Object.keys(def()?.properties || {}).length === 0}>
                              <p class="text-sm text-gray-600">No properties defined.</p>
                            </Show>
                            <Show when={Object.keys(def()?.properties || {}).length > 0}>
                              <div class="rounded-lg border border-[#141b28] overflow-hidden">
                                <table class="w-full text-left">
                                  <thead>
                                    <tr class="bg-[#090e1a]">
                                      <th class="py-2.5 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider">Name</th>
                                      <th class="py-2.5 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider">Type</th>
                                      <th class="py-2.5 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Required</th>
                                      <th class="py-2.5 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider">Description</th>
                                    </tr>
                                  </thead>
                                  <tbody>
                                    <For each={Object.entries(def()?.properties || {})}>
                                      {([propName, prop]) => (
                                        <tr class="border-t border-[#0e1521] hover:bg-white/[0.02] transition-colors">
                                          <td class="py-2.5 px-4 font-mono text-sm text-gray-300">
                                            {propName}
                                            <Show when={prop.required}><span class="text-red-400 ml-0.5">*</span></Show>
                                          </td>
                                          <td class="py-2.5 px-4">
                                            <Show when={prop.ref_name}>
                                              <span
                                                class="px-1.5 py-0.5 rounded text-xs font-mono bg-purple-500/10 text-purple-400 cursor-pointer hover:underline"
                                                onClick={() => selectDef(prop.ref_name!)}
                                              >{prop.ref_name}</span>
                                            </Show>
                                            <Show when={prop.is_array && prop.items_ref}>
                                              <span
                                                class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400 cursor-pointer hover:underline"
                                                onClick={() => selectDef(prop.items_ref!)}
                                              >[{prop.items_ref}]</span>
                                            </Show>
                                            <Show when={prop.is_array && !prop.items_ref}>
                                              <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400">[{prop.type}]</span>
                                            </Show>
                                            <Show when={prop.enum_values}>
                                              <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-pink-500/10 text-pink-400">enum</span>
                                            </Show>
                                            <Show when={!prop.ref_name && !prop.is_array && !prop.enum_values}>
                                              <span class={`px-1.5 py-0.5 rounded text-xs font-mono ${typeBadgeClass(prop.type, false, false)}`}>
                                                {prop.type}{prop.format ? ` (${prop.format})` : ""}
                                              </span>
                                            </Show>
                                            <Show when={prop.enum_values}>
                                              <div class="flex flex-wrap gap-1 mt-1">
                                                <For each={prop.enum_values!}>
                                                  {(val) => (
                                                    <span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-pink-500/5 text-pink-300">{val}</span>
                                                  )}
                                                </For>
                                              </div>
                                            </Show>
                                          </td>
                                          <td class="py-2.5 px-4 text-center">
                                            <span class={`text-sm ${prop.required ? "text-green-400" : "text-gray-700"}`}>
                                              {prop.required ? "\u2713" : "\u2014"}
                                            </span>
                                          </td>
                                          <td class="py-2.5 px-4 text-sm text-gray-500">
                                            {prop.description || "\u2014"}
                                          </td>
                                        </tr>
                                      )}
                                    </For>
                                  </tbody>
                                </table>
                              </div>
                            </Show>
                          </div>

                          {/* Used by endpoints */}
                          <Show when={eps().length > 0}>
                            <div class="px-6 py-4 border-t border-[#141b28]">
                              <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Used by endpoints</p>
                              <div class="space-y-1.5">
                                <For each={eps()}>
                                  {(route) => (
                                    <div class="flex items-center gap-2">
                                      <MethodBadge method={route.method} />
                                      <span class="font-mono text-sm text-gray-400">{route.path}</span>
                                    </div>
                                  )}
                                </For>
                              </div>
                            </div>
                          </Show>
                        </div>
                      );
                    })()}
                  </Show>
                </div>
              </div>
            </Show>
          </Show>

          {/* === Recipes === */}
          <Show when={page() === "recipes"}>
            <div class="flex items-center justify-between mb-6">
              <h2 class="text-2xl font-semibold">Recipes</h2>
              <Show when={!recipeCreating()}>
                <button
                  class="px-4 py-2 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors"
                  onClick={() => setRecipeCreating(true)}
                >
                  Create Recipe
                </button>
              </Show>
              <Show when={recipeCreating()}>
                <button
                  class="px-3.5 py-1.5 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                  onClick={handleRecipeCancelCreate}
                >
                  Cancel
                </button>
              </Show>
            </div>

            <Show when={recipeCreating()}>
              <StepIndicator current={recipeStep()} onNavigate={(s) => { if (saveActivatePhase() === "idle") setRecipeStep(s); }} />

              {/* Step 1: Paste spec */}
              <Show when={recipeStep() === "paste"}>
                <p class="text-gray-500 mb-4">Paste a Swagger 2.0 spec to create a recipe.</p>
                <textarea
                  class="w-full h-48 bg-[#070c17] border border-gray-800 rounded-lg p-4 font-mono text-sm text-gray-300 resize-y placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700 transition-colors"
                  placeholder="Paste Swagger 2.0 YAML or JSON here..."
                  value={recipeSpecText()}
                  onInput={(e) => setRecipeSpecText(e.currentTarget.value)}
                />
                <button
                  class="mt-4 px-5 py-2.5 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
                  onClick={handleRecipeParseSpec}
                  disabled={loading()}
                >
                  {loading() ? "Parsing..." : "Next: Select Endpoints"}
                </button>
              </Show>

              {/* Step 2: Select endpoints */}
              <Show when={recipeStep() === "select"}>
                <div class="flex items-center justify-between mb-4">
                  <p class="text-gray-500">Select endpoints for this recipe.</p>
                  <div class="flex gap-2">
                    <button
                      class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                      onClick={() => setRecipeSelectedEndpoints(recipeAvailableEndpoints().map(() => true))}
                    >
                      Select All
                    </button>
                    <button
                      class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                      onClick={() => setRecipeSelectedEndpoints(recipeAvailableEndpoints().map(() => false))}
                    >
                      Deselect All
                    </button>
                  </div>
                </div>
                <div class="space-y-0.5 mb-6">
                  <For each={recipeAvailableEndpoints()}>
                    {(ep, i) => (
                      <label class="flex items-center gap-3 px-3 py-2.5 rounded-md cursor-pointer hover:bg-white/[0.03] transition-colors">
                        <input
                          type="checkbox"
                          checked={recipeSelectedEndpoints()[i()]}
                          onChange={() => toggleRecipeEndpoint(i())}
                          class="accent-blue-500 rounded"
                        />
                        <MethodBadge method={ep.method} />
                        <span class="font-mono text-sm text-gray-300">{ep.path}</span>
                      </label>
                    )}
                  </For>
                </div>
                <button
                  class="px-5 py-2.5 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
                  onClick={handleFetchGraph}
                  disabled={graphLoading()}
                >
                  {graphLoading() ? "Computing graph..." : "Next: Entity Graph"}
                </button>
              </Show>

              {/* Step 3: Entity Graph */}
              <Show when={recipeStep() === "graph"}>
                <h3 class="text-lg font-semibold mb-2">Entity Graph</h3>
                <p class="text-sm text-gray-400 mb-4">Definitions reachable from your selected endpoints. Shared entities are highlighted.</p>
                <div class="space-y-2">
                  <For each={entityGraph()?.nodes || []}>
                    {(node: string) => (
                      <div class={`p-3 rounded ${entityGraph()?.shared_entities?.includes(node) ? 'bg-yellow-900/30 border border-yellow-700' : 'bg-gray-800'}`}>
                        <div class="flex items-center justify-between">
                          <span class="font-medium">{node}</span>
                          {entityGraph()?.shared_entities?.includes(node) && (
                            <span class="text-xs bg-yellow-700 px-2 py-0.5 rounded">shared</span>
                          )}
                        </div>
                        {entityGraph()?.roots?.[node] && (
                          <div class="text-xs text-gray-400 mt-1">
                            Root for: {entityGraph().roots[node].map((e: any) => `${e.method.toUpperCase()} ${e.path}`).join(", ")}
                          </div>
                        )}
                        {entityGraph()?.edges?.[node]?.length > 0 && (
                          <div class="text-xs text-gray-500 mt-1">
                            References: {entityGraph().edges[node].join(", ")}
                          </div>
                        )}
                      </div>
                    )}
                  </For>
                </div>
                <div class="flex gap-3 mt-4">
                  <button
                    class="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors"
                    onClick={() => setRecipeStep("select")}
                  >
                    Back
                  </button>
                  <button
                    class="px-5 py-2.5 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors"
                    onClick={handleGoToConfig}
                  >
                    Next: Configure
                  </button>
                </div>
              </Show>

              {/* Step 4: Configure data generation */}
              <Show when={recipeStep() === "config"}>
                <div>
                  <h3 class="text-lg font-semibold mb-2">Configure Data Generation</h3>

                  {Object.keys(recipeSharedPools()).length > 0 && (
                    <div class="mb-6">
                      <h4 class="text-sm font-medium text-gray-300 mb-2">Shared Entity Pools</h4>
                      <p class="text-sm text-gray-400 mb-3">Shared entities generate a fixed pool of instances reused across endpoints.</p>
                      <For each={Object.entries(recipeSharedPools())}>
                        {([entity, config]) => (
                          <div class="flex items-center gap-3 p-2 bg-gray-800 rounded mb-2">
                            <label class="flex items-center gap-2">
                              <input type="checkbox" checked={config.is_shared}
                                class="accent-blue-500 rounded"
                                onChange={(e) => {
                                  const pools = {...recipeSharedPools()};
                                  pools[entity] = {...pools[entity], is_shared: e.target.checked};
                                  setRecipeSharedPools(pools);
                                }} />
                              <span class="text-sm text-gray-200">{entity}</span>
                            </label>
                            <input type="number" min="1" max="100" value={config.pool_size}
                              class="w-20 bg-[#070c17] border border-gray-800 rounded-md px-2 py-1 text-sm text-gray-100 focus:outline-none focus:border-gray-700"
                              onInput={(e) => {
                                const pools = {...recipeSharedPools()};
                                pools[entity] = {...pools[entity], pool_size: parseInt(e.target.value) || 10};
                                setRecipeSharedPools(pools);
                              }} />
                            <span class="text-xs text-gray-500">instances</span>
                          </div>
                        )}
                      </For>
                    </div>
                  )}

                  {Object.keys(recipeQuantityConfigs()).length > 0 && (
                    <div class="mb-6">
                      <h4 class="text-sm font-medium text-gray-300 mb-2">Array Quantity Ranges</h4>
                      <p class="text-sm text-gray-400 mb-3">Control how many items are generated for array properties.</p>
                      <For each={Object.entries(recipeQuantityConfigs())}>
                        {([key, config]) => (
                          <div class="flex items-center gap-3 p-2 bg-gray-800 rounded mb-2">
                            <span class="font-mono text-sm text-gray-200 min-w-48">{key}</span>
                            <input type="number" min="0" max="50" value={config.min}
                              class="w-16 bg-[#070c17] border border-gray-800 rounded-md px-2 py-1 text-sm text-gray-100 focus:outline-none focus:border-gray-700"
                              onInput={(e) => {
                                const configs = {...recipeQuantityConfigs()};
                                configs[key] = {...configs[key], min: parseInt(e.target.value) || 0};
                                setRecipeQuantityConfigs(configs);
                              }} />
                            <span class="text-gray-500 text-sm">to</span>
                            <input type="number" min="1" max="50" value={config.max}
                              class="w-16 bg-[#070c17] border border-gray-800 rounded-md px-2 py-1 text-sm text-gray-100 focus:outline-none focus:border-gray-700"
                              onInput={(e) => {
                                const configs = {...recipeQuantityConfigs()};
                                configs[key] = {...configs[key], max: parseInt(e.target.value) || 3};
                                setRecipeQuantityConfigs(configs);
                              }} />
                            <span class="text-xs text-gray-500">items per record</span>
                          </div>
                        )}
                      </For>
                    </div>
                  )}

                  {Object.keys(recipeSharedPools()).length === 0 && Object.keys(recipeQuantityConfigs()).length === 0 && (
                    <p class="text-gray-400 mb-4">No shared entities or array properties detected. You can proceed to name your recipe.</p>
                  )}

                  <div class="flex gap-3 mt-4">
                    <button
                      class="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors"
                      onClick={() => setRecipeStep("graph")}
                    >
                      Back
                    </button>
                    <button
                      class="px-5 py-2.5 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors"
                      onClick={() => setRecipeStep("name")}
                    >
                      Next: Name & Save
                    </button>
                  </div>
                </div>
              </Show>

              {/* Step 5: Name + seed count + save & activate */}
              <Show when={recipeStep() === "name"}>
                <div class="space-y-4">
                  <div>
                    <label class="block text-sm text-gray-400 mb-1.5">Recipe Name</label>
                    <input
                      type="text"
                      value={recipeName()}
                      onInput={(e) => setRecipeName(e.currentTarget.value)}
                      placeholder="e.g., Petstore Dev"
                      class="w-full bg-[#070c17] border border-gray-800 rounded-lg px-4 py-2.5 text-sm text-gray-100 placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700 transition-colors"
                    />
                  </div>
                  <div>
                    <label class="block text-sm text-gray-400 mb-1.5">Seed rows per table</label>
                    <input
                      type="number"
                      value={recipeSeedCount()}
                      min={1}
                      max={100}
                      onInput={(e) => setRecipeSeedCount(parseInt(e.currentTarget.value) || 1)}
                      class="w-24 bg-[#070c17] border border-gray-800 rounded-md px-2.5 py-2 text-sm text-gray-100 focus:outline-none focus:border-gray-700"
                    />
                  </div>

                  <Show when={saveActivatePhase() !== "idle" && loading()}>
                    <div class="flex items-center gap-3 px-4 py-3 rounded-md bg-blue-500/10 border border-blue-500/20 text-blue-400 text-sm">
                      <div class="w-4 h-4 border-2 border-blue-400 border-t-transparent rounded-full animate-spin" />
                      {saveActivatePhase() === "saving" ? "Saving recipe..." : "Activating mock server..."}
                    </div>
                  </Show>

                  <Show when={savedRecipeId() !== null && !loading()}>
                    <div class="flex items-center gap-3 px-4 py-3 rounded-md bg-amber-500/10 border border-amber-500/20 text-amber-400 text-sm">
                      Recipe saved. Activation failed.
                      <button
                        class="ml-auto px-3 py-1 bg-amber-600 hover:bg-amber-500 rounded-md text-xs font-medium text-white transition-colors"
                        onClick={() => activateSavedRecipe(savedRecipeId()!)}
                      >
                        Retry Activate
                      </button>
                    </div>
                  </Show>

                  <div class="flex gap-3 pt-2">
                    <button
                      class="px-5 py-2.5 bg-green-600 hover:bg-green-500 rounded-lg text-sm font-medium transition-colors disabled:opacity-50"
                      onClick={handleRecipeSaveAndActivate}
                      disabled={loading()}
                    >
                      {loading()
                        ? saveActivatePhase() === "saving"
                          ? "Saving..."
                          : "Activating..."
                        : editingRecipeId() !== null
                          ? "Save"
                          : "Save & Activate"}
                    </button>
                    <button
                      class="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors disabled:opacity-50"
                      onClick={() => setRecipeStep("config")}
                      disabled={loading()}
                    >
                      Back
                    </button>
                  </div>
                </div>
              </Show>
            </Show>

            <Show when={!recipeCreating()}>
              <Show when={recipes().length === 0}>
                <p class="text-gray-500">No saved recipes yet. Create one to get started.</p>
              </Show>

              <Show when={recipes().length > 0}>
                <div class="space-y-3">
                  <For each={recipes()}>
                    {(recipe) => {
                      const endpoints: Endpoint[] = (() => {
                        try { return JSON.parse(recipe.selected_endpoints); } catch { return []; }
                      })();
                      return (
                        <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-5 flex items-center justify-between">
                          <div>
                            <p class="font-semibold text-gray-100">{recipe.name}</p>
                            <p class="text-sm text-gray-500 mt-0.5">
                              {endpoints.length} endpoint{endpoints.length !== 1 ? "s" : ""} &middot; {recipe.seed_count} seed rows &middot; {new Date(recipe.created_at).toLocaleDateString()}
                            </p>
                          </div>
                          <div class="flex gap-2">
                            <button
                              class="px-4 py-1.5 bg-green-600 hover:bg-green-500 rounded-md text-xs font-medium transition-colors disabled:opacity-50"
                              onClick={() => handleRecipeActivate(recipe.id)}
                              disabled={loading()}
                            >
                              Activate
                            </button>
                            <button
                              class="px-3 py-1.5 text-xs font-medium text-blue-400 hover:text-blue-300 border border-blue-500/20 hover:border-blue-500/40 rounded-md transition-colors disabled:opacity-50"
                              onClick={() => handleRecipeEdit(recipe)}
                              disabled={loading()}
                            >
                              Edit
                            </button>
                            <button
                              class="px-3 py-1.5 text-xs font-medium text-red-400 hover:text-red-300 border border-red-500/20 hover:border-red-500/40 rounded-md transition-colors"
                              onClick={() => handleRecipeDelete(recipe.id)}
                            >
                              Delete
                            </button>
                          </div>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </Show>
            </Show>
          </Show>

          {/* === Log === */}
          <Show when={page() === "log"}>
            <div class="flex items-center justify-between mb-6">
              <h2 class="text-2xl font-semibold">Request Log</h2>
              <span class="text-xs text-gray-500">{logEntries().length} entries &middot; auto-refreshing</span>
            </div>
            <Show when={logEntries().length === 0}>
              <p class="text-gray-500">No requests logged yet. Make some API calls to see them here.</p>
            </Show>
            <Show when={logEntries().length > 0}>
              <div class="rounded-xl border border-[#141b28] overflow-hidden">
                <table class="w-full text-left">
                  <thead>
                    <tr class="bg-[#090e1a]">
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider w-48">Time</th>
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider w-24">Method</th>
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider">Path</th>
                      <th class="py-3 px-4 text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={[...logEntries()].reverse()}>
                      {(entry) => (
                        <tr class="border-t border-[#0e1521]">
                          <td class="py-2 px-4 font-mono text-xs text-gray-500">{formatTime(entry.timestamp)}</td>
                          <td class="py-2 px-4"><MethodBadge method={entry.method} /></td>
                          <td class="py-2 px-4 font-mono text-sm text-gray-300">{entry.path}</td>
                          <td class="py-2 px-4"><StatusBadge status={entry.status} /></td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>
          </Show>
        </div>
      </main>
    </div>
  );
}

function MethodBadge(props: { method: string }) {
  const colors: Record<string, string> = {
    get: "bg-emerald-500/15 text-emerald-400 ring-emerald-500/20",
    post: "bg-blue-500/15 text-blue-400 ring-blue-500/20",
    delete: "bg-red-500/15 text-red-400 ring-red-500/20",
    put: "bg-amber-500/15 text-amber-400 ring-amber-500/20",
    patch: "bg-violet-500/15 text-violet-400 ring-violet-500/20",
  };
  const cls = colors[props.method.toLowerCase()] || "bg-gray-500/15 text-gray-400 ring-gray-500/20";
  return (
    <span class={`inline-block font-mono text-xs font-medium px-2 py-0.5 rounded ring-1 ${cls}`}>
      {props.method.toUpperCase()}
    </span>
  );
}

function StatusBadge(props: { status: number }) {
  const s = props.status;
  const cls = s >= 200 && s < 300
    ? "text-emerald-400"
    : s >= 400 && s < 500
    ? "text-amber-400"
    : s >= 500
    ? "text-red-400"
    : "text-gray-400";
  return <span class={`font-mono text-xs font-medium ${cls}`}>{s}</span>;
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString("en-US", { hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" })
      + "." + String(d.getMilliseconds()).padStart(3, "0");
  } catch {
    return iso;
  }
}

function rawValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "object") return JSON.stringify(value, null, 2);
  return String(value);
}

function formatCell(value: unknown): string {
  if (value === null || value === undefined) return "\u2014";
  if (Array.isArray(value)) return `[${value.length} items]`;
  if (typeof value === "object") {
    const obj = value as Record<string, unknown>;
    // Try common label fields
    for (const key of ["name", "title", "label", "id"]) {
      if (key in obj && (typeof obj[key] === "string" || typeof obj[key] === "number")) {
        return String(obj[key]);
      }
    }
    // Compact summary: show first few scalar values
    const parts: string[] = [];
    for (const [k, v] of Object.entries(obj)) {
      if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") {
        parts.push(`${k}: ${v}`);
      }
      if (parts.length >= 3) break;
    }
    return parts.length > 0 ? parts.join(", ") : `{${Object.keys(obj).length} fields}`;
  }
  if (typeof value === "string" && value.length > 80) return value.slice(0, 77) + "...";
  return String(value);
}

function StepIndicator(props: {
  current: "paste" | "select" | "graph" | "config" | "name";
  onNavigate: (step: "paste" | "select" | "graph" | "config" | "name") => void;
}) {
  const steps: { key: "paste" | "select" | "graph" | "config" | "name"; label: string }[] = [
    { key: "paste", label: "Import" },
    { key: "select", label: "Endpoints" },
    { key: "graph", label: "Entity Graph" },
    { key: "config", label: "Configure" },
    { key: "name", label: "Generate" },
  ];
  const stepIndex = (key: string) => steps.findIndex((s) => s.key === key);
  const currentIdx = () => stepIndex(props.current);

  return (
    <div class="flex items-center mb-8">
      <For each={steps}>
        {(step, i) => {
          const isActive = () => i() === currentIdx();
          const isCompleted = () => i() < currentIdx();
          const isFuture = () => i() > currentIdx();
          return (
            <>
              <button
                class={`flex items-center gap-2 px-3 py-1.5 rounded-md text-sm font-medium transition-colors ${
                  isActive()
                    ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                    : isCompleted()
                    ? "text-gray-300 hover:text-white hover:bg-white/5 cursor-pointer"
                    : "text-gray-600 cursor-default"
                }`}
                onClick={() => { if (isCompleted()) props.onNavigate(step.key); }}
                disabled={isFuture()}
              >
                <span class={`flex items-center justify-center w-5 h-5 rounded-full text-xs font-bold ${
                  isActive()
                    ? "bg-blue-600 text-white"
                    : isCompleted()
                    ? "bg-green-600 text-white"
                    : "bg-gray-800 text-gray-600"
                }`}>
                  {isCompleted() ? "\u2713" : i() + 1}
                </span>
                {step.label}
              </button>
              {i() < steps.length - 1 && (
                <div class={`flex-1 h-px mx-1 ${i() < currentIdx() ? "bg-green-600/40" : "bg-gray-800"}`} />
              )}
            </>
          );
        }}
      </For>
    </div>
  );
}

render(() => <App />, document.getElementById("root")!);
