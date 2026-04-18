import { render } from "solid-js/web";
import type { Accessor, Setter } from "solid-js";
import { createSignal, onMount, onCleanup, For, Index, Show, createEffect, createMemo, batch } from "solid-js";
import "./index.css";
import EntityBox, { ROW_HEIGHT, HEADER_HEIGHT } from "./EntityBox";
import { computeDagPositions } from "./dagLayout";

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
  request_body?: string;
  response_body?: string;
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
  faker_rules: string;
  rules: string;
  frozen_rows: string;
}

// Recipe rule data model — mirrors the Rust serde tagged union in src/rules.rs.
// The `kind` discriminator is snake_case; CompareOp is also snake_case.
type CompareOp = "eq" | "neq" | "gt" | "gte" | "lt" | "lte";
type RuleKind = "range" | "choice" | "const" | "pattern" | "compare";

type RangeRule = { kind: "range"; field: string; min: number; max: number };
type ChoiceRule = { kind: "choice"; field: string; options: (string | number | boolean)[] };
type ConstRule = { kind: "const"; field: string; value: string | number | boolean };
type PatternRule = { kind: "pattern"; field: string; regex: string };
type CompareRule = { kind: "compare"; left: string; op: CompareOp; right: string | number | boolean };

type Rule = RangeRule | ChoiceRule | ConstRule | PatternRule | CompareRule;

const RULE_KINDS: RuleKind[] = ["range", "choice", "const", "pattern", "compare"];
const COMPARE_OPS: CompareOp[] = ["eq", "neq", "gt", "gte", "lt", "lte"];

// Pure helpers for constraint rule value parsing (hoisted from ConstraintRulesEditor)
const parseLiteral = (raw: string): string | number | boolean => {
  const trimmed = raw.trim();
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (trimmed !== "" && !isNaN(Number(trimmed))) return Number(trimmed);
  return raw;
};
const parseChoiceOptions = (raw: string): (string | number | boolean)[] => {
  if (!raw.trim()) return [];
  return raw.split(",").map((s) => parseLiteral(s.trim()));
};
const stringifyChoiceOptions = (opts: (string | number | boolean)[]): string => {
  return opts.map((o) => String(o)).join(", ");
};

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

  // Try-it-out state
  const [tryEndpoint, setTryEndpoint] = createSignal<string | null>(null);
  const [tryParams, setTryParams] = createSignal<Record<string, string>>({});
  const [tryBody, setTryBody] = createSignal("{}");
  const [tryResponse, setTryResponse] = createSignal<{ status: number; body: string } | null>(null);
  const [trySending, setTrySending] = createSignal(false);
  const [endpointFilter, setEndpointFilter] = createSignal("");
  const [endpointMethodFilter, setEndpointMethodFilter] = createSignal<string | null>(null);
  const [selectingFilter, setSelectingFilter] = createSignal("");
  const [selectingMethodFilter, setSelectingMethodFilter] = createSignal<string | null>(null);
  const filteredEndpoints = () => {
    const q = endpointFilter().toLowerCase();
    const m = endpointMethodFilter();
    return activeEndpoints().filter((ep) =>
      (!q || ep.path.toLowerCase().includes(q)) &&
      (!m || ep.method.toLowerCase() === m)
    );
  };
  const filteredAvailableEndpoints = createMemo(() => {
    const q = selectingFilter().toLowerCase();
    const m = selectingMethodFilter();
    return availableEndpoints()
      .map((ep, index) => ({ ep, originalIndex: index }))
      .filter(({ ep }) =>
        (!q || ep.path.toLowerCase().includes(q)) &&
        (!m || ep.method.toLowerCase() === m)
      );
  });

  const [tables, setTables] = createSignal<TableInfo[]>([]);
  const [selectedTable, setSelectedTable] = createSignal<string | null>(null);
  const [tableData, setTableData] = createSignal<TableData | null>(null);
  const [tableLoading, setTableLoading] = createSignal(false);
  const [tableFilter, setTableFilter] = createSignal("");
  const [showEmptyTables, setShowEmptyTables] = createSignal(false);
  const [selectedCellValue, setSelectedCellValue] = createSignal<any>(null);
  const [editingCell, setEditingCell] = createSignal<{rowid: string|number, col: string} | null>(null);
  const filteredTables = () => {
    const q = tableFilter().toLowerCase();
    const all = showEmptyTables() ? tables() : tables().filter((t) => t.row_count > 0);
    return q ? all.filter((t) => t.name.toLowerCase().includes(q)) : all;
  };
  const emptyTableNames = createMemo(() => {
    const s = new Set<string>();
    for (const t of tables()) {
      if (t.row_count === 0) s.add(t.name);
    }
    return s;
  });
  const allTableNames = createMemo(() => new Set(tables().map(t => t.name)));

  // Log state
  const [logEntries, setLogEntries] = createSignal<LogEntry[]>([]);
  const [selectedLog, setSelectedLog] = createSignal<LogEntry | null>(null);
  const [hideInternalCalls, setHideInternalCalls] = createSignal(false);
  const displayedEntries = createMemo(() => {
    const entries = hideInternalCalls()
      ? logEntries().filter(e => !e.path.startsWith("/_api/"))
      : logEntries();
    return [...entries].reverse();
  });

  // Recipe state
  const [recipes, setRecipes] = createSignal<Recipe[]>([]);
  const [recipeCreating, setRecipeCreating] = createSignal(false);
  const [recipeSpecText, setRecipeSpecText] = createSignal("");
  const [recipeName, setRecipeName] = createSignal("");
  const [recipeSeedCount, setRecipeSeedCount] = createSignal(10);
  const [recipeAvailableEndpoints, setRecipeAvailableEndpoints] = createSignal<Endpoint[]>([]);
  const [recipeSelectedEndpoints, setRecipeSelectedEndpoints] = createSignal<boolean[]>([]);
  const [recipeStep, setRecipeStep] = createSignal<"paste" | "select" | "config" | "name">("paste");
  const [entityGraph, setEntityGraph] = createSignal<any>(null);
  const [graphLoading, setGraphLoading] = createSignal(false);
  const [recipeSharedPools, setRecipeSharedPools] = createSignal<Record<string, {is_shared: boolean, pool_size: number}>>({});
  const [recipeQuantityConfigs, setRecipeQuantityConfigs] = createSignal<Record<string, {min: number, max: number}>>({});
  const [recipeFakerRules, setRecipeFakerRules] = createSignal<Record<string, string>>({});
  const [recipeRules, setRecipeRules] = createSignal<Rule[]>([]);
  const [configSearch, setConfigSearch] = createSignal("");
  const [configShowNonDefault, setConfigShowNonDefault] = createSignal(false);
  const [editingRecipeId, setEditingRecipeId] = createSignal<number | null>(null);
  const [activeRecipeId, setActiveRecipeId] = createSignal<number | null>(null);
  const [frozenRows, setFrozenRows] = createSignal<Record<string, Record<string, unknown>[]>>({});

  // Schema state
  const [definitions, setDefinitions] = createSignal<Record<string, DefinitionInfo>>({});
  const [schemaRoutes, setSchemaRoutes] = createSignal<RouteInfo[]>([]);
  const [expandedDefs, setExpandedDefs] = createSignal<Set<string>>(new Set());
  const [selectedEntities, setSelectedEntities] = createSignal<Set<string>>(new Set());
  const [lastSelectedEntity, setLastSelectedEntity] = createSignal<string | null>(null);
  const [schemaFilter, setSchemaFilter] = createSignal("");

  // Schema-level entity graph state
  const [schemaGraph, setSchemaGraph] = createSignal<any>(null);
  const [schemaGraphGroupBy, setSchemaGraphGroupBy] = createSignal<"alpha" | "endpoint">("alpha");
  const [schemaGraphHopDepth, setSchemaGraphHopDepth] = createSignal(1);

  // Progressive exploration graph state
  const [graphFocused, setGraphFocused] = createSignal<string | null>(null);
  const [graphExpanded, setGraphExpanded] = createSignal<Set<string>>(new Set());
  const [graphPan, setGraphPan] = createSignal<{ x: number; y: number }>({ x: 0, y: 0 });
  const [graphZoom, setGraphZoom] = createSignal<number>(1);

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
    if (page() === "tables" || page() === "dashboard" || page() === "schemas") {
      refreshTables();
    }
  });

  createEffect(() => {
    if (page() === "tables") {
      const recipeId = activeRecipeId();
      if (recipeId !== null) {
        (async () => {
          try {
            const res = await fetch(`/_api/admin/recipes/${recipeId}/config`);
            if (res.ok) {
              const config = await res.json();
              setFrozenRows(config.frozen_rows || {});
            }
          } catch {
            // ignore — frozen rows just won't be available
          }
        })();
      } else {
        setFrozenRows({});
      }
    }
  });

  createEffect(() => {
    selectedTable();
    setSelectedCellValue(null);
    setEditingCell(null);
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
          const [defRes, routesRes, graphRes] = await Promise.all([
            fetch("/_api/admin/definitions"),
            fetch("/_api/admin/routes"),
            fetch("/_api/admin/graph"),
          ]);
          setDefinitions(await defRes.json());
          setSchemaRoutes(await routesRes.json());
          setSchemaGraph(await graphRes.json());
        } catch {
          // ignore
        }
      })();
    }
  });

  createEffect(() => {
    if (page() === "endpoints" && Object.keys(definitions()).length === 0) {
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

  const putCellValue = async (rowid: string | number, column: string, value: unknown): Promise<boolean> => {
    try {
      const res = await fetch(`/_api/admin/tables/${encodeURIComponent(selectedTable()!)}/${rowid}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ [column]: value }),
      });
      return res.ok;
    } catch {
      return false;
    }
  };

  const toggleFreezeRow = async (tableName: string, row: Record<string, unknown>) => {
    const recipeId = activeRecipeId();
    if (recipeId === null) return;

    // First fetch current config to get all fields
    let currentConfig: Record<string, unknown>;
    try {
      const configRes = await fetch(`/_api/admin/recipes/${recipeId}/config`);
      if (!configRes.ok) return;
      currentConfig = await configRes.json();
    } catch {
      return;
    }

    const currentFrozen = frozenRows();
    const tableRows = currentFrozen[tableName] || [];

    // Build row snapshot without rowid
    const rowSnapshot: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(row)) {
      if (key !== "rowid") rowSnapshot[key] = val;
    }

    // Check if this row is already frozen (match by all non-rowid fields)
    const existingIndex = tableRows.findIndex((fr) => {
      if (Object.keys(fr).length !== Object.keys(rowSnapshot).length) return false;
      for (const key of Object.keys(rowSnapshot)) {
        if (JSON.stringify(fr[key]) !== JSON.stringify(rowSnapshot[key])) return false;
      }
      return true;
    });

    let updatedTableRows: Record<string, unknown>[];
    if (existingIndex >= 0) {
      // Unfreeze: remove the row
      updatedTableRows = tableRows.filter((_, i) => i !== existingIndex);
    } else {
      // Freeze: add the row
      updatedTableRows = [...tableRows, rowSnapshot];
    }

    const updatedFrozen = { ...currentFrozen };
    if (updatedTableRows.length === 0) {
      delete updatedFrozen[tableName];
    } else {
      updatedFrozen[tableName] = updatedTableRows;
    }

    // PUT to backend with full config
    try {
      const res = await fetch(`/_api/admin/recipes/${recipeId}/config`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          shared_pools: currentConfig.shared_pools || {},
          quantity_configs: currentConfig.quantity_configs || {},
          faker_rules: currentConfig.faker_rules || {},
          rules: currentConfig.rules || [],
          frozen_rows: updatedFrozen,
        }),
      });
      if (res.ok) {
        setFrozenRows(updatedFrozen);
      }
    } catch {
      // ignore — don't update local state on failure
    }
  };

  const isRowFrozen = (tableName: string, row: Record<string, unknown>): boolean => {
    const tableRows = frozenRows()[tableName] || [];
    const rowSnapshot: Record<string, unknown> = {};
    for (const [key, val] of Object.entries(row)) {
      if (key !== "rowid") rowSnapshot[key] = val;
    }
    return tableRows.some((fr) => {
      if (Object.keys(fr).length !== Object.keys(rowSnapshot).length) return false;
      for (const key of Object.keys(rowSnapshot)) {
        if (JSON.stringify(fr[key]) !== JSON.stringify(rowSnapshot[key])) return false;
      }
      return true;
    });
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
      setSelectingFilter("");
      setSelectingMethodFilter(null);
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

  // selected[] is always indexed against the full availableEndpoints() array,
  // not the filtered view. When filtering is active, use originalIndex to map back.
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
      setActiveRecipeId(null);
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
      const sortedEndpoints = [...data.endpoints].sort((a: Endpoint, b: Endpoint) =>
        a.path.localeCompare(b.path) || a.method.localeCompare(b.method)
      );
      setRecipeAvailableEndpoints(sortedEndpoints);
      setRecipeSelectedEndpoints(sortedEndpoints.map(() => true));
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
      handleGoToConfig();
    } catch (e: any) {
      setError(String(e?.message || e));
    } finally {
      setGraphLoading(false);
    }
  };

  const handleGoToConfig = () => {
    const graph = entityGraph();
    if (!graph) return;

    // Preserve existing config if already populated (edit mode tab switching)
    if (Object.keys(recipeSharedPools()).length === 0) {
      const pools: Record<string, {is_shared: boolean, pool_size: number}> = {};
      for (const entity of graph.shared_entities || []) {
        pools[entity] = { is_shared: true, pool_size: 10 };
      }
      setRecipeSharedPools(pools);
    }

    if (Object.keys(recipeQuantityConfigs()).length === 0) {
      const configs: Record<string, {min: number, max: number}> = {};
      for (const ap of graph.array_properties || []) {
        configs[`${ap.def_name}.${ap.prop_name}`] = { min: 1, max: 3 };
      }
      setRecipeQuantityConfigs(configs);
    }

    if (Object.keys(recipeFakerRules()).length === 0) {
      const rules: Record<string, string> = {};
      for (const sp of graph.scalar_properties || []) {
        rules[`${sp.def_name}.${sp.prop_name}`] = "auto";
      }
      setRecipeFakerRules(rules);
    }

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
            faker_rules: recipeFakerRules(),
            rules: recipeRules(),
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
        setRecipeFakerRules({});
        setRecipeRules([]);
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
          faker_rules: recipeFakerRules(),
          rules: recipeRules(),
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
      setActiveRecipeId(id);
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
      setRecipeFakerRules({});
      setRecipeRules([]);
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
      setActiveRecipeId(id);
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

  const handleRecipeClone = async (id: number) => {
    setError(null);
    try {
      const res = await fetch(`/_api/admin/recipes/${id}/clone`, {
        method: "POST",
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
      if (id === activeRecipeId()) setActiveRecipeId(null);
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
      const availableEps: Endpoint[] = [...data.endpoints].sort((a, b) =>
        a.path.localeCompare(b.path) || a.method.localeCompare(b.method)
      );

      // Parse selected endpoints from recipe
      let selectedEps: Endpoint[] = [];
      try { selectedEps = JSON.parse(recipe.selected_endpoints); } catch { /* empty */ }

      // Map to boolean array: true if the available endpoint is in the selected list
      const selectedFlags = availableEps.map((ep) =>
        selectedEps.some((sel) => sel.method === ep.method && sel.path === ep.path)
      );

      // Parse shared_pools, quantity_configs, faker_rules, and rules
      let sharedPools: Record<string, {is_shared: boolean, pool_size: number}> = {};
      try { sharedPools = JSON.parse(recipe.shared_pools); } catch { /* empty */ }
      let quantityConfigs: Record<string, {min: number, max: number}> = {};
      try { quantityConfigs = JSON.parse(recipe.quantity_configs); } catch { /* empty */ }
      let fakerRules: Record<string, string> = {};
      try { fakerRules = JSON.parse(recipe.faker_rules); } catch { /* empty */ }
      let rules: Rule[] = [];
      try {
        const parsed = JSON.parse(recipe.rules ?? "[]");
        if (Array.isArray(parsed)) rules = parsed as Rule[];
      } catch { /* empty */ }

      setRecipeSpecText(recipe.spec_source);
      setRecipeAvailableEndpoints(availableEps);
      setRecipeSelectedEndpoints(selectedFlags);
      setRecipeName(recipe.name);
      setRecipeSeedCount(recipe.seed_count);
      setRecipeSharedPools(sharedPools);
      setRecipeQuantityConfigs(quantityConfigs);
      setRecipeFakerRules(fakerRules);
      setRecipeRules(rules);
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
    setRecipeFakerRules({});
    setRecipeRules([]);
    setEditingRecipeId(null);
    setError(null);
  };

  const handleRecipeExport = async (id: number) => {
    try {
      const res = await fetch(`/_api/admin/recipes/${id}/export`);
      if (!res.ok) {
        const text = await res.text();
        try { setError(JSON.parse(text).error || text); } catch { setError(`${res.status}: ${text}`); }
        return;
      }
      const blob = await res.blob();
      const disposition = res.headers.get("content-disposition") || "";
      const match = disposition.match(/filename="(.+)"/);
      const filename = match ? match[1] : "recipe.mirage.json";
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filename;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e: any) {
      setError(String(e?.message || e));
    }
  };

  const handleRecipeImport = async () => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".json,.mirage.json";
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      setLoading(true);
      setError(null);
      try {
        const text = await file.text();
        const data = JSON.parse(text);
        const res = await fetch("/_api/admin/recipes/import", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(data),
        });
        if (!res.ok) {
          const rtext = await res.text();
          try { setError(JSON.parse(rtext).error || rtext); } catch { setError(`${res.status}: ${rtext}`); }
          setLoading(false);
          return;
        }
        await refreshRecipes();
      } catch (e: any) {
        setError(String(e?.message || e));
      }
      setLoading(false);
    };
    input.click();
  };

  const toggleDef = (name: string) => {
    const next = new Set(expandedDefs());
    if (next.has(name)) next.delete(name);
    else next.add(name);
    setExpandedDefs(next);
  };

  const selectEntity = (name: string) => {
    const prev = selectedEntities();
    const next = new Set(prev);
    const wasSelected = next.has(name);
    if (wasSelected) {
      next.delete(name);
    } else {
      next.add(name);
    }
    setSelectedEntities(next);
    if (wasSelected) {
      const remaining = Array.from(next);
      setLastSelectedEntity(remaining.length > 0 ? remaining[remaining.length - 1] : null);
    } else {
      setLastSelectedEntity(name);
      if (!expandedDefs().has(name)) toggleDef(name);
    }
    // Progressive exploration: focus the clicked entity in the graph (only on select, not deselect)
    if (!wasSelected && graphFocused() !== name) {
      batch(() => {
        setGraphFocused(name);
        // Pre-populate expanded set with all immediate neighbors so they render with fields visible
        const defs = definitions();
        const neighbors = new Set<string>();
        const focusedDef = defs[name];
        if (focusedDef) {
          if (focusedDef.extends && defs[focusedDef.extends] && focusedDef.extends !== name) {
            neighbors.add(focusedDef.extends);
          }
          for (const prop of Object.values(focusedDef.properties)) {
            const ref = prop.ref_name || prop.items_ref;
            if (ref && defs[ref] && ref !== name) neighbors.add(ref);
          }
        }
        for (const [entityName, def] of Object.entries(defs)) {
          if (entityName === name) continue;
          if (def.extends === name) { neighbors.add(entityName); continue; }
          for (const prop of Object.values(def.properties)) {
            const ref = prop.ref_name || prop.items_ref;
            if (ref === name) { neighbors.add(entityName); break; }
          }
        }
        setGraphExpanded(neighbors);
      });
    }
  };

  const collapseGraph = () => {
    batch(() => {
      setSelectedEntities(new Set<string>());
      setLastSelectedEntity(null);
      setGraphFocused(null);
      setGraphExpanded(new Set<string>());
    });
  };

  const endpointsForDef = (defName: string) =>
    schemaRoutes().filter(r => r.definition === defName);

  const examplePayload = (ep: Endpoint): string => {
    const route = schemaRoutes().find(
      (r) => r.method.toLowerCase() === ep.method.toLowerCase() && r.path === ep.path
    );
    if (!route) return "{}";
    const def = definitions()[route.definition];
    if (!def) return "{}";

    const buildExample = (defName: string, depth: number): Record<string, unknown> => {
      if (depth > 2) return {};
      const d = definitions()[defName];
      if (!d) return {};

      let obj: Record<string, unknown> = {};
      if (d.extends) {
        obj = buildExample(d.extends, depth + 1);
      }

      for (const [name, prop] of Object.entries(d.properties)) {
        if (name === "id") continue;

        if (prop.enum_values && prop.enum_values.length > 0) {
          obj[name] = prop.enum_values[0];
        } else if (prop.is_array) {
          if (prop.items_ref) {
            obj[name] = [buildExample(prop.items_ref, depth + 1)];
          } else {
            obj[name] = [];
          }
        } else if (prop.ref_name) {
          obj[name] = buildExample(prop.ref_name, depth + 1);
        } else {
          switch (prop.type) {
            case "string":
              if (prop.format === "date-time") obj[name] = "2026-01-01T00:00:00Z";
              else if (prop.format === "date") obj[name] = "2026-01-01";
              else if (prop.format === "email") obj[name] = "user@example.com";
              else if (prop.format === "uri" || prop.format === "url") obj[name] = "https://example.com";
              else obj[name] = "string";
              break;
            case "integer": obj[name] = 0; break;
            case "number": obj[name] = 0.0; break;
            case "boolean": obj[name] = true; break;
            default: obj[name] = "string"; break;
          }
        }
      }
      return obj;
    };

    return JSON.stringify(buildExample(route.definition, 0), null, 2);
  };

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
          <h1 class="text-2xl font-semibold tracking-tight flex items-center gap-2.5">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" shape-rendering="crispEdges" class="w-9 h-9 shrink-0">
              <rect width="16" height="16" fill="#0d1117" rx="3"/>
              <rect x="7" y="1" width="1" height="1" fill="#fde68a"/>
              <rect x="6" y="2" width="1" height="1" fill="#fbbf24"/>
              <rect x="7" y="2" width="1" height="1" fill="#f59e0b"/>
              <rect x="5" y="3" width="1" height="1" fill="#fbbf24"/>
              <rect x="6" y="3" width="2" height="1" fill="#f59e0b"/>
              <rect x="5" y="4" width="4" height="1" fill="#f59e0b"/>
              <rect x="5" y="4" width="1" height="1" fill="#fbbf24"/>
              <rect x="6" y="5" width="4" height="1" fill="#f59e0b"/>
              <rect x="7" y="5" width="1" height="1" fill="#fbbf24"/>
              <rect x="7" y="6" width="4" height="1" fill="#f59e0b"/>
              <rect x="6" y="7" width="4" height="1" fill="#f59e0b"/>
              <rect x="6" y="7" width="1" height="1" fill="#92400e"/>
              <rect x="5" y="8" width="5" height="1" fill="#f59e0b"/>
              <rect x="5" y="8" width="1" height="1" fill="#92400e"/>
              <rect x="4" y="9" width="5" height="1" fill="#f59e0b"/>
              <rect x="4" y="9" width="1" height="1" fill="#92400e"/>
              <rect x="8" y="9" width="1" height="1" fill="#92400e"/>
              <rect x="5" y="10" width="5" height="1" fill="#f59e0b"/>
              <rect x="9" y="10" width="1" height="1" fill="#92400e"/>
              <rect x="6" y="11" width="4" height="1" fill="#f59e0b"/>
              <rect x="7" y="12" width="2" height="1" fill="#92400e"/>
              <rect x="8" y="13" width="1" height="1" fill="#92400e"/>
              <rect x="9" y="1" width="1" height="1" fill="#fde68a"/>
              <rect x="3" y="6" width="1" height="1" fill="#fde68a"/>
              <rect x="12" y="8" width="1" height="1" fill="#fde68a"/>
              <rect x="5" y="14" width="1" height="1" fill="#fde68a"/>
            </svg>
            Mirage
          </h1>
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
      <main class="flex-1 h-screen flex flex-col overflow-hidden">
        <div class="flex-1 min-h-0 flex flex-col px-8 pt-8 pb-3 overflow-y-auto">
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
                <div class="flex items-center gap-3">
                  <span class="text-xs text-gray-500">{filteredAvailableEndpoints().length} / {availableEndpoints().length}</span>
                  <div class="flex gap-2">
                    <button
                      class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                      onClick={() => setSelected(prev => {
                        const next = [...prev];
                        filteredAvailableEndpoints().forEach(({ originalIndex }) => {
                          next[originalIndex] = true;
                        });
                        return next;
                      })}
                    >
                      Select All
                    </button>
                    <button
                      class="px-3 py-1 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                      onClick={() => setSelected(prev => {
                        const next = [...prev];
                        filteredAvailableEndpoints().forEach(({ originalIndex }) => {
                          next[originalIndex] = false;
                        });
                        return next;
                      })}
                    >
                      Deselect All
                    </button>
                  </div>
                </div>
              </div>
              <div class="flex items-center gap-2 mb-4">
                <input
                  type="text"
                  placeholder="Filter by path..."
                  value={selectingFilter()}
                  onInput={(e) => setSelectingFilter(e.currentTarget.value)}
                  class="flex-1 bg-[#070c17] border border-gray-800 rounded-md px-3 py-1.5 text-sm text-gray-100 font-mono placeholder:text-gray-600 focus:outline-none focus:border-gray-600"
                />
                <For each={["get", "post", "put", "delete", "patch"]}>
                  {(m) => {
                    const active = () => selectingMethodFilter() === m;
                    return (
                      <button
                        class={`px-2 py-1 text-xs font-mono rounded-md border transition-colors ${active() ? "border-gray-600 bg-white/[0.06] text-gray-200" : "border-gray-800 text-gray-500 hover:text-gray-400 hover:border-gray-700"}`}
                        onClick={() => setSelectingMethodFilter(active() ? null : m)}
                      >
                        {m.toUpperCase()}
                      </button>
                    );
                  }}
                </For>
              </div>
              <div id="endpoint-list" class="space-y-0.5 mb-6">
                <For each={filteredAvailableEndpoints()}>
                  {(item) => (
                    <label class="flex items-center gap-3 px-3 py-2.5 rounded-md cursor-pointer hover:bg-white/[0.03] transition-colors">
                      <input
                        type="checkbox"
                        checked={selected()[item.originalIndex]}
                        onChange={() => toggleEndpoint(item.originalIndex)}
                        class="endpoint-checkbox accent-blue-500 rounded"
                      />
                      <MethodBadge method={item.ep.method} />
                      <span class="font-mono text-sm text-gray-300">{item.ep.path}</span>
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
              <div class="flex items-center justify-between mb-4">
                <h2 class="text-2xl font-semibold">Active Endpoints</h2>
                <span class="text-xs text-gray-500">{filteredEndpoints().length} / {activeEndpoints().length}</span>
              </div>
              <div class="flex items-center gap-2 mb-4">
                <input
                  type="text"
                  placeholder="Filter by path..."
                  value={endpointFilter()}
                  onInput={(e) => setEndpointFilter(e.currentTarget.value)}
                  class="flex-1 bg-[#070c17] border border-gray-800 rounded-md px-3 py-1.5 text-sm text-gray-100 font-mono placeholder:text-gray-600 focus:outline-none focus:border-gray-600"
                />
                <For each={["get", "post", "put", "delete", "patch"]}>
                  {(m) => {
                    const active = () => endpointMethodFilter() === m;
                    return (
                      <button
                        class={`px-2 py-1 text-xs font-mono rounded-md border transition-colors ${active() ? "border-gray-600 bg-white/[0.06] text-gray-200" : "border-gray-800 text-gray-500 hover:text-gray-400 hover:border-gray-700"}`}
                        onClick={() => setEndpointMethodFilter(active() ? null : m)}
                      >
                        {m.toUpperCase()}
                      </button>
                    );
                  }}
                </For>
              </div>
              <div class="space-y-1">
                <For each={filteredEndpoints()}>
                  {(ep) => {
                    const key = () => `${ep.method}:${ep.path}`;
                    const isOpen = () => tryEndpoint() === key();
                    const pathParams = () => {
                      const matches = ep.path.match(/\{(\w+)\}/g);
                      return matches ? matches.map((m) => m.slice(1, -1)) : [];
                    };
                    const needsBody = () => ["post", "put", "patch"].includes(ep.method.toLowerCase());

                    const resolvedPath = () => {
                      let p = ep.path;
                      const params = tryParams();
                      for (const param of pathParams()) {
                        p = p.replace(`{${param}}`, params[param] || `{${param}}`);
                      }
                      return p;
                    };

                    const sendRequest = async () => {
                      setTrySending(true);
                      setTryResponse(null);
                      try {
                        const opts: RequestInit = { method: ep.method.toUpperCase() };
                        if (needsBody()) {
                          opts.headers = { "Content-Type": "application/json" };
                          opts.body = tryBody();
                        }
                        const res = await fetch(resolvedPath(), opts);
                        const text = await res.text();
                        let formatted = text;
                        try { formatted = JSON.stringify(JSON.parse(text), null, 2); } catch {}
                        setTryResponse({ status: res.status, body: formatted });
                      } catch (e: any) {
                        setTryResponse({ status: 0, body: e.message || "Request failed" });
                      }
                      setTrySending(false);
                    };

                    return (
                      <div class="rounded-lg border border-[#141b28] overflow-hidden">
                        <button
                          class={`w-full flex items-center gap-3 px-4 py-2.5 text-left hover:bg-white/[0.03] transition-colors ${isOpen() ? "bg-white/[0.02]" : ""}`}
                          onClick={() => {
                            if (isOpen()) {
                              setTryEndpoint(null);
                            } else {
                              setTryEndpoint(key());
                              setTryParams({});
                              setTryBody(["post", "put", "patch"].includes(ep.method.toLowerCase()) ? examplePayload(ep) : "{}");
                              setTryResponse(null);
                            }
                          }}
                        >
                          <svg class={`w-3 h-3 text-gray-500 transition-transform ${isOpen() ? "rotate-90" : ""}`} fill="currentColor" viewBox="0 0 8 12"><path d="M1.5 0L0 1.5 4.5 6 0 10.5 1.5 12l6-6z"/></svg>
                          <MethodBadge method={ep.method} />
                          <span class="font-mono text-sm text-gray-300">{ep.path}</span>
                        </button>

                        <Show when={isOpen()}>
                          <div class="px-4 pb-4 pt-2 border-t border-[#141b28] space-y-3">
                            <Show when={pathParams().length > 0}>
                              <div class="space-y-2">
                                <For each={pathParams()}>
                                  {(param) => (
                                    <div class="flex items-center gap-2">
                                      <label class="text-xs text-gray-500 w-24 text-right font-mono">{`{${param}}`}</label>
                                      <input
                                        type="text"
                                        placeholder={param}
                                        value={tryParams()[param] || ""}
                                        onInput={(e) => setTryParams({ ...tryParams(), [param]: e.currentTarget.value })}
                                        class="flex-1 bg-[#070c17] border border-gray-800 rounded-md px-2.5 py-1.5 text-sm text-gray-100 font-mono focus:outline-none focus:border-gray-600"
                                      />
                                    </div>
                                  )}
                                </For>
                              </div>
                            </Show>

                            <Show when={needsBody()}>
                              <div>
                                <label class="text-xs text-gray-500 block mb-1">Request Body</label>
                                <textarea
                                  value={tryBody()}
                                  onInput={(e) => setTryBody(e.currentTarget.value)}
                                  rows={5}
                                  class="w-full bg-[#070c17] border border-gray-800 rounded-md px-3 py-2 text-sm text-gray-100 font-mono focus:outline-none focus:border-gray-600 resize-y"
                                  spellcheck={false}
                                />
                              </div>
                            </Show>

                            <div class="flex items-center gap-3">
                              <span class="text-xs text-gray-500 font-mono">{ep.method.toUpperCase()} {resolvedPath()}</span>
                              <button
                                class="px-4 py-1.5 bg-blue-600 hover:bg-blue-500 rounded-md text-xs font-medium transition-colors disabled:opacity-50"
                                onClick={sendRequest}
                                disabled={trySending()}
                              >
                                {trySending() ? "Sending..." : "Send"}
                              </button>
                            </div>

                            <Show when={tryResponse()}>
                              {(res) => (
                                <div class="rounded-md border border-[#141b28] overflow-hidden">
                                  <div class="flex items-center gap-2 px-3 py-1.5 bg-[#090e1a] border-b border-[#141b28]">
                                    <span class="text-xs text-gray-500">Status</span>
                                    <span class={`text-xs font-mono font-medium ${res().status >= 200 && res().status < 300 ? "text-emerald-400" : res().status >= 400 ? "text-red-400" : "text-gray-300"}`}>
                                      {res().status || "ERR"}
                                    </span>
                                  </div>
                                  <pre class="px-3 py-2 text-xs text-gray-300 font-mono overflow-x-auto max-h-72 overflow-y-auto whitespace-pre-wrap">{res().body}</pre>
                                </div>
                              )}
                            </Show>
                          </div>
                        </Show>
                      </div>
                    );
                  }}
                </For>
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
                <div class="flex gap-0">
                  {/* Table list */}
                  <div class="shrink-0 border-r border-[#141b28] pr-2 flex flex-col max-h-[calc(100vh-12rem)]">
                    <div class="pb-2 space-y-1.5">
                      <input
                        type="text"
                        placeholder="Filter tables..."
                        value={tableFilter()}
                        onInput={(e) => setTableFilter(e.currentTarget.value)}
                        class="w-full px-3 py-1.5 text-sm bg-white/5 border border-[#141b28] rounded-md text-gray-300 placeholder-gray-600 focus:outline-none focus:border-blue-500/40"
                      />
                      <Show when={emptyTableNames().size > 0}>
                        <button
                          class={`px-2.5 py-1 text-xs rounded-md transition-colors ${
                            showEmptyTables()
                              ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                              : "text-gray-500 hover:text-gray-300"
                          }`}
                          onClick={() => {
                            const wasShowing = showEmptyTables();
                            setShowEmptyTables(!wasShowing);
                            if (wasShowing) {
                              const sel = selectedTable();
                              if (sel && emptyTableNames().has(sel)) {
                                setSelectedTable(null);
                                setTableData(null);
                              }
                            }
                          }}
                        >{showEmptyTables() ? "Hide empty" : "Show empty"}</button>
                      </Show>
                    </div>
                    <div class="overflow-y-auto space-y-0.5">
                      <For each={filteredTables()}>
                        {(t) => (
                          <button
                            class={`w-full flex items-center justify-between px-3 py-2 rounded-md text-sm whitespace-nowrap transition-all ${
                              selectedTable() === t.name
                                ? "bg-blue-500/15 text-blue-300 font-medium border-l-2 border-blue-400"
                                : t.row_count === 0
                                  ? "text-gray-600 hover:text-gray-400 hover:bg-white/5"
                                  : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                            }`}
                            onClick={() => loadTableData(t.name)}
                          >
                            <span>{t.name}</span>
                            <span class={`text-xs tabular-nums ml-3 shrink-0 ${
                              t.row_count === 0 ? "opacity-30" : "opacity-50"
                            }`}>{t.row_count}</span>
                          </button>
                        )}
                      </For>
                    </div>
                  </div>

                  {/* Table data */}
                  <div class="flex-1 min-w-0 pl-6">
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
                                <For each={tableData()!.columns.filter(c => c.name !== "rowid")}>
                                  {(col) => (
                                    <th class="py-3 px-4 text-xs font-medium text-gray-500 whitespace-nowrap" title={col.name}>
                                      {humanizeColumn(col.name)}
                                    </th>
                                  )}
                                </For>
                                <Show when={activeRecipeId() !== null}>
                                  <th class="py-3 px-4 text-xs font-medium text-gray-500 whitespace-nowrap"></th>
                                </Show>
                              </tr>
                            </thead>
                            <tbody>
                              <For each={tableData()!.rows}>
                                {(row) => {
                                  const frozen = () => selectedTable() ? isRowFrozen(selectedTable()!, row as Record<string, unknown>) : false;
                                  return (
                                  <tr class={`border-t border-[#0e1521] transition-colors ${frozen() ? "bg-blue-500/[0.08]" : "hover:bg-white/[0.02]"}`}>
                                    <For each={tableData()!.columns.filter(c => c.name !== "rowid")}>
                                      {(col) => {
                                        const val = row[col.name];
                                        const isComplex = typeof val === "object" && val !== null;
                                        const editing = () => {
                                          const ec = editingCell();
                                          return ec !== null && ec.rowid === row.rowid && ec.col === col.name;
                                        };
                                        const commitEdit = async (newValue: string) => {
                                          let coerced: unknown = newValue;
                                          if (typeof val === "number" && newValue.trim() !== "" && !isNaN(Number(newValue))) {
                                            coerced = Number(newValue);
                                          } else if (typeof val === "boolean") {
                                            if (newValue.toLowerCase() === "true") coerced = true;
                                            else if (newValue.toLowerCase() === "false") coerced = false;
                                          }
                                          const ok = await putCellValue(row.rowid, col.name, coerced);
                                          if (ok) {
                                            setTableData(prev => {
                                              if (!prev) return prev;
                                              return {
                                                ...prev,
                                                rows: prev.rows.map(r =>
                                                  r.rowid === row.rowid ? { ...r, [col.name]: coerced } : r
                                                ),
                                              };
                                            });
                                          }
                                        };
                                        return (
                                          <td
                                            class={`py-2.5 px-4 font-mono text-xs whitespace-nowrap max-w-[200px] truncate ${isComplex ? "text-blue-400 underline decoration-blue-400/40 cursor-pointer hover:text-blue-300" : "text-gray-300"}`}
                                            title={rawValue(val)}
                                            onClick={isComplex ? () => setSelectedCellValue(val) : !editing() ? () => setEditingCell({ rowid: row.rowid as string | number, col: col.name }) : undefined}
                                          >
                                            {editing() ? (
                                              <input
                                                type="text"
                                                value={rawValue(val)}
                                                ref={(el) => setTimeout(() => el.focus(), 0)}
                                                class="bg-[#070c17] border border-gray-700 rounded px-2 py-0.5 text-xs text-gray-200 font-mono w-full focus:outline-none focus:border-blue-500"
                                                onBlur={(e) => {
                                                  if (!editingCell()) return;
                                                  commitEdit(e.currentTarget.value);
                                                  setEditingCell(prev =>
                                                    prev?.rowid === row.rowid && prev?.col === col.name ? null : prev
                                                  );
                                                }}
                                                onKeyDown={(e) => {
                                                  if (e.key === "Enter") {
                                                    commitEdit(e.currentTarget.value);
                                                    setEditingCell(null);
                                                  } else if (e.key === "Escape") {
                                                    setEditingCell(null);
                                                  }
                                                }}
                                                onClick={(e) => e.stopPropagation()}
                                              />
                                            ) : (
                                              formatCell(val)
                                            )}
                                          </td>
                                        );
                                      }}
                                    </For>
                                    <Show when={activeRecipeId() !== null}>
                                      <td class="py-2.5 px-4 whitespace-nowrap">
                                        <button
                                          class={`text-xs px-2 py-1 rounded transition-colors ${
                                            frozen()
                                              ? "bg-blue-500/20 text-blue-300 hover:bg-blue-500/30"
                                              : "bg-gray-700/30 text-gray-500 hover:bg-gray-700/50 hover:text-gray-300"
                                          }`}
                                          onClick={() => selectedTable() && toggleFreezeRow(selectedTable()!, row as Record<string, unknown>)}
                                        >
                                          {frozen() ? "Unfreeze" : "Freeze"}
                                        </button>
                                      </td>
                                    </Show>
                                  </tr>
                                  );
                                }}
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
            <SchemasPage
              schemaGraph={schemaGraph}
              definitions={definitions}
              selectedEntities={selectedEntities}
              lastSelectedEntity={lastSelectedEntity}
              collapseGraph={collapseGraph}
              schemaFilter={schemaFilter}
              setSchemaFilter={setSchemaFilter}
              schemaGraphGroupBy={schemaGraphGroupBy}
              setSchemaGraphGroupBy={setSchemaGraphGroupBy}
              schemaGraphHopDepth={schemaGraphHopDepth}
              setSchemaGraphHopDepth={setSchemaGraphHopDepth}
              expandedDefs={expandedDefs}
              toggleDef={toggleDef}
              selectEntity={selectEntity}
              endpointsForDef={endpointsForDef}
              typeBadgeClass={typeBadgeClass}
              graphFocused={graphFocused}
              setGraphFocused={setGraphFocused}
              graphExpanded={graphExpanded}
              setGraphExpanded={setGraphExpanded}
              graphPan={graphPan}
              setGraphPan={setGraphPan}
              graphZoom={graphZoom}
              setGraphZoom={setGraphZoom}
              emptyTableNames={emptyTableNames}
              allTableNames={allTableNames}
              showEmptyTables={showEmptyTables}
              setShowEmptyTables={setShowEmptyTables}
              clearEmptyEntities={() => {
                const empty = emptyTableNames();
                const allTables = allTableNames();
                batch(() => {
                  const sel = selectedEntities();
                  const stale = [...sel].filter(n => allTables.has(n) && empty.has(n));
                  if (stale.length > 0) {
                    const next = new Set(sel);
                    for (const n of stale) next.delete(n);
                    setSelectedEntities(next);
                    const last = lastSelectedEntity();
                    if (last && allTables.has(last) && empty.has(last)) {
                      const remaining = Array.from(next);
                      setLastSelectedEntity(remaining.length > 0 ? remaining[remaining.length - 1] : null);
                    }
                  }
                  const exp = expandedDefs();
                  const staleExp = [...exp].filter(n => allTables.has(n) && empty.has(n));
                  if (staleExp.length > 0) {
                    const nextExp = new Set(exp);
                    for (const n of staleExp) nextExp.delete(n);
                    setExpandedDefs(nextExp);
                  }
                  const focused = graphFocused();
                  if (focused && allTables.has(focused) && empty.has(focused)) {
                    setGraphFocused(null);
                    setGraphExpanded(new Set<string>());
                  }
                });
              }}
            />
          </Show>

          {/* === Recipes === */}
          <Show when={page() === "recipes"}>
            <div class="flex items-center justify-between mb-6">
              <h2 class="text-2xl font-semibold">Recipes</h2>
              <Show when={!recipeCreating()}>
                <div class="flex gap-2">
                  <button
                    class="px-4 py-2 bg-blue-600 hover:bg-blue-500 rounded-lg text-sm font-medium transition-colors"
                    onClick={() => setRecipeCreating(true)}
                  >
                    Create Recipe
                  </button>
                  <button
                    class="px-4 py-2 text-sm font-medium text-gray-300 hover:text-gray-100 border border-gray-700 hover:border-gray-600 rounded-lg transition-colors"
                    onClick={handleRecipeImport}
                  >
                    Import
                  </button>
                </div>
              </Show>
              <Show when={recipeCreating()}>
                <div class="flex items-center gap-2">
                  {/* Back button */}
                  <Show when={recipeStep() !== "paste"}>
                    <button
                      class="px-3.5 py-1.5 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors disabled:opacity-50"
                      disabled={loading()}
                      onClick={() => {
                        const prev: Record<string, "paste" | "select" | "config"> = { select: "paste", config: "select", name: "config" };
                        setRecipeStep(prev[recipeStep()] || "paste");
                      }}
                    >Back</button>
                  </Show>
                  {/* Next button */}
                  <Show when={recipeStep() !== "name"}>
                    <button
                      class="px-3.5 py-1.5 text-xs font-medium bg-blue-600 hover:bg-blue-500 rounded-md transition-colors disabled:opacity-50"
                      disabled={loading() || graphLoading()}
                      onClick={() => {
                        if (recipeStep() === "paste") handleRecipeParseSpec();
                        else if (recipeStep() === "select") handleFetchGraph();
                        else if (recipeStep() === "config") setRecipeStep("name");
                      }}
                    >
                      {recipeStep() === "paste" ? (loading() ? "Parsing..." : "Next")
                        : recipeStep() === "select" ? (graphLoading() ? "Computing..." : "Next")
                        : "Next"}
                    </button>
                  </Show>
                  {/* Save button on last step */}
                  <Show when={recipeStep() === "name"}>
                    <button
                      class="px-3.5 py-1.5 text-xs font-medium bg-green-600 hover:bg-green-500 rounded-md transition-colors disabled:opacity-50"
                      onClick={handleRecipeSaveAndActivate}
                      disabled={loading()}
                    >
                      {loading()
                        ? saveActivatePhase() === "saving" ? "Saving..." : "Activating..."
                        : editingRecipeId() !== null ? "Save" : "Save & Activate"}
                    </button>
                  </Show>
                  <div class="w-px h-5 bg-gray-800 mx-1" />
                  <button
                    class="px-3.5 py-1.5 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-800 hover:border-gray-700 rounded-md transition-colors"
                    onClick={handleRecipeCancelCreate}
                  >Cancel</button>
                </div>
              </Show>
            </div>

            <Show when={recipeCreating()}>
              <StepIndicator
                current={recipeStep()}
                editMode={editingRecipeId() !== null}
                onNavigate={async (s) => {
                  if (saveActivatePhase() !== "idle") return;
                  if (s === "config") {
                    if (!entityGraph()) {
                      await handleFetchGraph();
                      return;
                    }
                    if (Object.keys(recipeSharedPools()).length === 0 && Object.keys(recipeQuantityConfigs()).length === 0) {
                      handleGoToConfig();
                      return;
                    }
                  }
                  setRecipeStep(s);
                }}
              />

              {/* Step 1: Paste spec */}
              <Show when={recipeStep() === "paste"}>
                <p class="text-gray-500 mb-4">Paste a Swagger 2.0 spec to create a recipe.</p>
                <textarea
                  class="w-full h-48 bg-[#070c17] border border-gray-800 rounded-lg p-4 font-mono text-sm text-gray-300 resize-y placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700 transition-colors"
                  placeholder="Paste Swagger 2.0 YAML or JSON here..."
                  value={recipeSpecText()}
                  onInput={(e) => setRecipeSpecText(e.currentTarget.value)}
                />
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
              </Show>

              {/* Step 3: Configure data generation */}
              <Show when={recipeStep() === "config"}>
                <RecipeConfigStep
                  recipeSharedPools={recipeSharedPools}
                  setRecipeSharedPools={setRecipeSharedPools}
                  recipeQuantityConfigs={recipeQuantityConfigs}
                  setRecipeQuantityConfigs={setRecipeQuantityConfigs}
                  recipeFakerRules={recipeFakerRules}
                  setRecipeFakerRules={setRecipeFakerRules}
                  recipeRules={recipeRules}
                  setRecipeRules={setRecipeRules}
                  entityGraph={entityGraph}
                  configSearch={configSearch}
                  setConfigSearch={setConfigSearch}
                  configShowNonDefault={configShowNonDefault}
                  setConfigShowNonDefault={setConfigShowNonDefault}
                />
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
                              class="px-3 py-1.5 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-600/30 hover:border-gray-500/50 rounded-md transition-colors"
                              onClick={() => handleRecipeExport(recipe.id)}
                            >
                              Export
                            </button>
                            <button
                              class="px-3 py-1.5 text-xs font-medium text-gray-400 hover:text-gray-200 border border-gray-600/30 hover:border-gray-500/50 rounded-md transition-colors disabled:opacity-50"
                              onClick={() => handleRecipeClone(recipe.id)}
                              disabled={loading()}
                            >
                              Clone
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
            <div class="flex flex-col flex-1 min-h-0">
              <div class="flex items-center justify-between mb-6 shrink-0">
                <h2 class="text-2xl font-semibold">Request Log</h2>
                <div class="flex items-center gap-4">
                  <label class="flex items-center gap-1.5 text-xs text-zinc-400 cursor-pointer select-none">
                    <input
                      type="checkbox"
                      checked={hideInternalCalls()}
                      onChange={(e) => setHideInternalCalls(e.currentTarget.checked)}
                      class="accent-blue-500"
                    />
                    Hide internal calls
                  </label>
                  <span class="text-xs text-gray-500">
                    {hideInternalCalls()
                      ? `${displayedEntries().length} / ${logEntries().length} entries`
                      : `${logEntries().length} entries`} &middot; auto-refreshing
                  </span>
                </div>
              </div>
              <Show when={logEntries().length === 0}>
                <p class="text-gray-500">No requests logged yet. Make some API calls to see them here.</p>
              </Show>
              <Show when={logEntries().length > 0 && displayedEntries().length === 0}>
                <p class="text-gray-500">All entries hidden by filter.</p>
              </Show>
              <Show when={displayedEntries().length > 0}>
                <div class="rounded-xl border border-[#141b28] overflow-hidden flex-1 min-h-0 overflow-y-auto">
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
                      <For each={displayedEntries()}>
                        {(entry) => (
                          <tr
                            class={`border-t border-[#0e1521] cursor-pointer transition-colors ${
                              selectedLog() === entry ? "bg-[#111827]" : "hover:bg-[#0c1220]"
                            }`}
                            onClick={() => setSelectedLog(selectedLog() === entry ? null : entry)}
                          >
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
            </div>
          </Show>
          <Show when={selectedLog()}>
            {(entry) => (
              <div class="fixed inset-0 z-50 flex items-center justify-center" onClick={() => setSelectedLog(null)}>
                <div class="absolute inset-0 bg-black/60" />
                <div class="relative bg-[#0a1020] border border-[#141b28] rounded-xl shadow-2xl w-[90vw] max-w-4xl max-h-[80vh] flex flex-col" onClick={(e) => e.stopPropagation()}>
                  <div class="flex items-center justify-between px-5 py-4 border-b border-[#141b28] shrink-0">
                    <div class="flex items-center gap-3">
                      <MethodBadge method={entry().method} />
                      <span class="font-mono text-sm text-gray-300">{entry().path}</span>
                      <StatusBadge status={entry().status} />
                      <span class="font-mono text-xs text-gray-500">{formatTime(entry().timestamp)}</span>
                    </div>
                    <button class="text-gray-500 hover:text-gray-300 text-lg leading-none px-1" onClick={() => setSelectedLog(null)}>&times;</button>
                  </div>
                  <div class="grid grid-cols-2 gap-4 p-5 overflow-auto min-h-0">
                    <div class="flex flex-col min-h-0">
                      <h4 class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-2 shrink-0">Request Body</h4>
                      <pre class="bg-[#070c17] rounded-lg p-3 text-xs text-gray-300 font-mono overflow-auto whitespace-pre-wrap flex-1">
                        {entry().request_body ? tryFormatJson(entry().request_body!) : <span class="text-gray-600 italic">No body</span>}
                      </pre>
                    </div>
                    <div class="flex flex-col min-h-0">
                      <h4 class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-2 shrink-0">Response Body</h4>
                      <pre class="bg-[#070c17] rounded-lg p-3 text-xs text-gray-300 font-mono overflow-auto whitespace-pre-wrap flex-1">
                        {entry().response_body ? tryFormatJson(entry().response_body!) : <span class="text-gray-600 italic">No body</span>}
                      </pre>
                    </div>
                  </div>
                </div>
              </div>
            )}
          </Show>
          <Show when={selectedCellValue()}>
            {(cellVal) => (
              <div class="fixed inset-0 z-50 flex items-center justify-center" onClick={() => setSelectedCellValue(null)}>
                <div class="absolute inset-0 bg-black/60" />
                <div class="relative bg-[#0a1020] border border-[#141b28] rounded-xl shadow-2xl w-[90vw] max-w-3xl max-h-[80vh] flex flex-col" onClick={(e) => e.stopPropagation()}>
                  <div class="flex items-center justify-between px-5 py-4 border-b border-[#141b28] shrink-0">
                    <span class="text-sm font-medium text-gray-300">{Array.isArray(cellVal()) ? "Array" : "Object"} Detail</span>
                    <button class="text-gray-500 hover:text-gray-300 text-lg leading-none px-1" onClick={() => setSelectedCellValue(null)}>&times;</button>
                  </div>
                  <div class="p-5 overflow-auto min-h-0">
                    <pre class="bg-[#070c17] rounded-lg p-4 text-xs text-gray-300 font-mono overflow-auto whitespace-pre-wrap">
                      {(() => { try { return JSON.stringify(cellVal(), null, 2); } catch { return "Unable to display value"; } })()}
                    </pre>
                  </div>
                </div>
              </div>
            )}
          </Show>
        </div>
      </main>
    </div>
  );
}

type GroupedFields = Record<string, { def: string; prop: string; type: string; format: string | null }[]>;

function FieldSelect(props: {
  value: string;
  onChange: (next: string) => void;
  groupedFields: Accessor<GroupedFields>;
}) {
  return (
    <select
      value={props.value}
      class="bg-[#070c17] border border-gray-800 rounded-md px-2 py-1 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
      onChange={(e) => props.onChange(e.target.value)}
    >
      <Show when={!props.value}>
        <option value="">-- field --</option>
      </Show>
      <For each={Object.entries(props.groupedFields()).sort(([a], [b]) => a.localeCompare(b))}>
        {([def, ps]) => (
          <optgroup label={def}>
            <For each={ps}>
              {(p) => (
                <option value={`${def}.${p.prop}`}>
                  {p.prop} <Show when={p.type}>({p.type}{p.format ? `/${p.format}` : ""})</Show>
                </option>
              )}
            </For>
          </optgroup>
        )}
      </For>
    </select>
  );
}

function SchemasPage(props: {
  schemaGraph: Accessor<any>;
  definitions: Accessor<Record<string, DefinitionInfo>>;
  selectedEntities: Accessor<Set<string>>;
  lastSelectedEntity: Accessor<string | null>;
  collapseGraph: () => void;
  schemaFilter: Accessor<string>;
  setSchemaFilter: Setter<string>;
  schemaGraphGroupBy: Accessor<"alpha" | "endpoint">;
  setSchemaGraphGroupBy: Setter<"alpha" | "endpoint">;
  schemaGraphHopDepth: Accessor<number>;
  setSchemaGraphHopDepth: Setter<number>;
  expandedDefs: Accessor<Set<string>>;
  toggleDef: (name: string) => void;
  selectEntity: (name: string) => void;
  endpointsForDef: (defName: string) => RouteInfo[];
  typeBadgeClass: (type: string, isRef: boolean, isEnum: boolean) => string;
  graphFocused: Accessor<string | null>;
  setGraphFocused: Setter<string | null>;
  graphExpanded: Accessor<Set<string>>;
  setGraphExpanded: Setter<Set<string>>;
  graphPan: Accessor<{ x: number; y: number }>;
  setGraphPan: Setter<{ x: number; y: number }>;
  graphZoom: Accessor<number>;
  setGraphZoom: Setter<number>;
  emptyTableNames: Accessor<Set<string>>;
  allTableNames: Accessor<Set<string>>;
  showEmptyTables: Accessor<boolean>;
  setShowEmptyTables: Setter<boolean>;
  clearEmptyEntities: () => void;
}) {
  const graph = () => props.schemaGraph();
  const nodes = () => (graph()?.nodes || []) as string[];
  const edges = () => (graph()?.edges || {}) as Record<string, string[]>;
  const roots = () => (graph()?.roots || {}) as Record<string, { method: string; path: string }[]>;
  const arrayTargets = () => [...new Set((graph()?.array_properties || []).map((ap: any) => ap.target_def))] as string[];
  const virtualRoots = () => (graph()?.virtual_roots || []) as { endpoint: { method: string; path: string }; shape: string }[];
  const [vrCollapsed, setVrCollapsed] = createSignal(true);
  const [rightTab, setRightTab] = createSignal<"details" | "graph">("details");

  const filteredNodes = () => {
    const q = props.schemaFilter().toLowerCase();
    const empty = props.emptyTableNames();
    const hideEmpty = !props.showEmptyTables() && empty.size > 0;
    const allNodes = nodes();
    const defKeys = Object.keys(props.definitions());
    const merged = [...new Set([...allNodes, ...defKeys])];
    let filtered = q ? merged.filter((n: string) => n.toLowerCase().includes(q)) : merged;
    if (hideEmpty) {
      const allTables = props.allTableNames();
      filtered = filtered.filter((n: string) => !allTables.has(n) || !empty.has(n));
    }
    if (props.schemaGraphGroupBy() === "alpha") return filtered.sort();
    return filtered;
  };

  const endpointGroups = () => {
    if (props.schemaGraphGroupBy() !== "endpoint") return {};
    const groups: Record<string, string[]> = {};
    const r = roots();
    const assigned = new Set<string>();
    for (const node of filteredNodes()) {
      if (r[node]) {
        for (const ep of r[node]) {
          const key = `${ep.method.toUpperCase()} ${ep.path}`;
          if (!groups[key]) groups[key] = [];
          groups[key].push(node);
          assigned.add(node);
        }
      }
    }
    const unrooted = filteredNodes().filter((n: string) => !assigned.has(n));
    if (unrooted.length > 0) groups["Referenced (no direct root)"] = unrooted;
    return groups;
  };

  const neighborhood = createMemo(() => {
    const entities = props.selectedEntities();
    const depth = props.schemaGraphHopDepth();
    if (entities.size === 0) return { nodes: [] as string[], edges: {} as Record<string, string[]>, roleMap: {} as Record<string, { role: 'focused' | 'parent' | 'child'; hop: number }> };
    const e = edges();

    const roleMap: Record<string, { role: 'focused' | 'parent' | 'child'; hop: number }> = {};

    // For each selected entity, run BFS and merge into roleMap
    for (const sel of entities) {
      // Mark focused — always wins
      if (!roleMap[sel] || roleMap[sel].role !== 'focused') {
        roleMap[sel] = { role: 'focused', hop: 0 };
      }

      // Outbound BFS (children)
      const outVisited = new Set<string>([sel]);
      let outFrontier = new Set<string>([sel]);
      for (let hop = 1; hop <= depth; hop++) {
        const next = new Set<string>();
        for (const n of outFrontier) {
          for (const t of (e[n] || [])) {
            if (!outVisited.has(t)) {
              outVisited.add(t);
              next.add(t);
              const existing = roleMap[t];
              if (!existing) {
                roleMap[t] = { role: 'child', hop };
              } else if (existing.role !== 'focused') {
                if (hop < existing.hop) {
                  roleMap[t] = { role: 'child', hop };
                }
              }
            }
          }
        }
        outFrontier = next;
      }

      // Inbound BFS (parents)
      const inVisited = new Set<string>([sel]);
      let inFrontier = new Set<string>([sel]);
      for (let hop = 1; hop <= depth; hop++) {
        const next = new Set<string>();
        for (const n of inFrontier) {
          for (const [src, targets] of Object.entries(e)) {
            if (targets.includes(n) && !inVisited.has(src)) {
              inVisited.add(src);
              next.add(src);
              const existing = roleMap[src];
              if (!existing) {
                roleMap[src] = { role: 'parent', hop };
              } else if (existing.role !== 'focused') {
                if (hop < existing.hop) {
                  roleMap[src] = { role: 'parent', hop };
                } else if (hop === existing.hop && existing.role === 'child') {
                  roleMap[src] = { role: 'parent', hop };
                }
              }
            }
          }
        }
        inFrontier = next;
      }
    }

    // Collect all nodes in roleMap
    const allNodes = Object.keys(roleMap);
    const visited = new Set(allNodes);
    const nbEdges: Record<string, string[]> = {};
    for (const n of allNodes) {
      const targets = (e[n] || []).filter(t => visited.has(t));
      if (targets.length > 0) nbEdges[n] = targets;
    }
    return { nodes: allNodes, edges: nbEdges, roleMap };
  });

  createEffect(() => {
    const sel = props.lastSelectedEntity();
    if (!sel) return;
    const id = requestAnimationFrame(() => {
      const el = document.querySelector(`[data-entity="${CSS.escape(sel)}"]`);
      if (el) el.scrollIntoView({ behavior: "smooth", block: "nearest" });
    });
    onCleanup(() => cancelAnimationFrame(id));
  });

  return (
    <div class="flex flex-col flex-1 min-h-0">
      <div class="flex items-center justify-between mb-6 shrink-0">
        <h2 class="text-2xl font-semibold">Schemas</h2>
      </div>
      <Show when={Object.keys(props.definitions()).length === 0}>
        <p class="text-gray-500">No definitions available. Import a spec first.</p>
      </Show>
      <Show when={Object.keys(props.definitions()).length > 0}>
        <div class="flex gap-6 flex-1 min-h-0">
          {/* Left panel - Hybrid entity list */}
          <div class="w-72 shrink-0 flex flex-col">
            <div class="rounded-xl bg-[#0a101d] border border-[#141b28] overflow-hidden flex flex-col h-full">
              <div class="px-4 py-3 border-b border-[#141b28] space-y-2">
                <div class="flex items-center justify-between">
                  <span class="text-xs font-medium text-gray-500 uppercase tracking-wider">Definitions</span>
                  <span class="text-xs text-gray-600 tabular-nums">{filteredNodes().length}</span>
                </div>
                <input
                  type="text"
                  placeholder="Filter..."
                  value={props.schemaFilter()}
                  onInput={(e) => props.setSchemaFilter(e.currentTarget.value)}
                  class="w-full bg-[#070c17] border border-gray-800 rounded-md px-2.5 py-1.5 text-xs text-gray-100 placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700"
                />
                <div class="flex gap-1 flex-wrap">
                  <button
                    class={`px-2.5 py-1 text-xs rounded-md transition-colors ${props.schemaGraphGroupBy() === "alpha" ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30" : "text-gray-400 hover:text-gray-200"}`}
                    onClick={() => props.setSchemaGraphGroupBy("alpha")}
                  >A--Z</button>
                  <button
                    class={`px-2.5 py-1 text-xs rounded-md transition-colors ${props.schemaGraphGroupBy() === "endpoint" ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30" : "text-gray-400 hover:text-gray-200"}`}
                    onClick={() => props.setSchemaGraphGroupBy("endpoint")}
                  >By endpoint</button>
                  <Show when={props.emptyTableNames().size > 0}>
                    <button
                      class={`px-2.5 py-1 text-xs rounded-md transition-colors ml-auto ${
                        props.showEmptyTables()
                          ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                          : "text-gray-500 hover:text-gray-300"
                      }`}
                      onClick={() => {
                        const wasShowing = props.showEmptyTables();
                        props.setShowEmptyTables(!wasShowing);
                        if (wasShowing) {
                          props.clearEmptyEntities();
                        }
                      }}
                    >{props.showEmptyTables() ? "Hide empty" : "Show empty"}</button>
                  </Show>
                </div>
              </div>
              <div class="flex-1 overflow-y-auto" style="scrollbar-width: thin;">
                <Show when={props.schemaGraphGroupBy() === "alpha"}>
                  <For each={filteredNodes()}>
                    {(defName: string) => {
                      const def = () => props.definitions()[defName];
                      const eps = () => props.endpointsForDef(defName);
                      const isSelected = () => props.selectedEntities().has(defName);
                      const isExpanded = () => props.expandedDefs().has(defName);
                      const isParent = () => Object.keys(roots()).includes(defName);
                      return (
                        <div data-entity={defName}>
                          <button
                            class={`w-full flex items-center gap-2 px-3 py-2 text-sm transition-all ${
                              isSelected()
                                ? "bg-blue-600/15 text-blue-400 font-medium ring-1 ring-blue-500/30"
                                : isParent()
                                ? "bg-yellow-900/20 text-gray-200 hover:bg-yellow-900/30 border-l-2 border-yellow-800/40"
                                : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                            }`}
                            onClick={() => props.selectEntity(defName)}
                          >
                            <svg
                              class={`w-3 h-3 shrink-0 transition-transform ${isExpanded() ? "rotate-90" : ""}`}
                              fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"
                              onClick={(e: MouseEvent) => { e.stopPropagation(); props.toggleDef(defName); }}
                            >
                              <path stroke-linecap="round" stroke-linejoin="round" d="M9 5l7 7-7 7" />
                            </svg>
                            <span class="truncate flex-1 text-left">{defName}</span>
                            <Show when={eps().length > 0}>
                              <span class="flex items-center gap-0.5 shrink-0">
                                <For each={eps().slice(0, 3)}>
                                  {(route) => (
                                    <span class={`inline-block font-mono text-[9px] font-medium px-1 py-0 rounded ring-1 ${
                                      ({
                                        get: "bg-emerald-500/15 text-emerald-400 ring-emerald-500/20",
                                        post: "bg-blue-500/15 text-blue-400 ring-blue-500/20",
                                        delete: "bg-red-500/15 text-red-400 ring-red-500/20",
                                        put: "bg-amber-500/15 text-amber-400 ring-amber-500/20",
                                        patch: "bg-violet-500/15 text-violet-400 ring-violet-500/20",
                                      } as Record<string, string>)[route.method.toLowerCase()] || "bg-gray-500/15 text-gray-400 ring-gray-500/20"
                                    }`}>{route.method.toUpperCase()}</span>
                                  )}
                                </For>
                                <Show when={eps().length > 3}>
                                  <span class="text-[9px] text-gray-500">+{eps().length - 3}</span>
                                </Show>
                              </span>
                            </Show>
                            {(edges()[defName]?.length || 0) > 0 && (
                              <span class="text-[10px] text-gray-600 shrink-0">{edges()[defName].length}</span>
                            )}
                          </button>
                          <Show when={def()?.extends}>
                            <div class="px-3 pl-8 pb-1">
                              <span class="text-xs text-gray-600">extends </span>
                              <span
                                class="text-xs text-purple-400 cursor-pointer hover:underline"
                                onClick={() => props.selectEntity(def()!.extends!)}
                              >{def()!.extends}</span>
                            </div>
                          </Show>
                          <Show when={isExpanded() && def()}>
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
                                        onClick={() => props.selectEntity(prop.ref_name!)}
                                      >{prop.ref_name}</span>
                                    </Show>
                                    <Show when={prop.is_array && prop.items_ref}>
                                      <span
                                        class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400 cursor-pointer hover:underline"
                                        onClick={() => props.selectEntity(prop.items_ref!)}
                                      >[{prop.items_ref}]</span>
                                    </Show>
                                    <Show when={prop.is_array && !prop.items_ref}>
                                      <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400">[{prop.type}]</span>
                                    </Show>
                                    <Show when={prop.enum_values}>
                                      <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-pink-500/10 text-pink-400">enum</span>
                                    </Show>
                                    <Show when={!prop.ref_name && !prop.is_array && !prop.enum_values}>
                                      <span class={`px-1.5 py-0.5 rounded text-xs font-mono ${props.typeBadgeClass(prop.type, false, false)}`}>{prop.type}</span>
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
                </Show>
                <Show when={props.schemaGraphGroupBy() === "endpoint"}>
                  <For each={Object.entries(endpointGroups())}>
                    {([endpoint, groupNodes]) => (
                      <div class="mb-2">
                        <div class="text-[10px] font-medium text-gray-500 px-2 py-1 sticky top-0 bg-[#0a101d] z-10">{endpoint}</div>
                        <For each={groupNodes as string[]}>
                          {(defName: string) => {
                            const def = () => props.definitions()[defName];
                            const eps = () => props.endpointsForDef(defName);
                            const isSelected = () => props.selectedEntities().has(defName);
                            const isExpanded = () => props.expandedDefs().has(defName);
                            const isParent = () => Object.keys(roots()).includes(defName);
                            return (
                              <div data-entity={defName}>
                                <button
                                  class={`w-full flex items-center gap-2 px-3 py-2 text-sm transition-all ${
                                    isSelected()
                                      ? "bg-blue-600/15 text-blue-400 font-medium ring-1 ring-blue-500/30"
                                      : isParent()
                                      ? "bg-yellow-900/20 text-gray-200 hover:bg-yellow-900/30 border-l-2 border-yellow-800/40"
                                      : "text-gray-400 hover:text-gray-200 hover:bg-white/5"
                                  }`}
                                  onClick={() => props.selectEntity(defName)}
                                >
                                  <svg
                                    class={`w-3 h-3 shrink-0 transition-transform ${isExpanded() ? "rotate-90" : ""}`}
                                    fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"
                                    onClick={(e: MouseEvent) => { e.stopPropagation(); props.toggleDef(defName); }}
                                  >
                                    <path stroke-linecap="round" stroke-linejoin="round" d="M9 5l7 7-7 7" />
                                  </svg>
                                  <span class="truncate flex-1 text-left">{defName}</span>
                                  <Show when={eps().length > 0}>
                                    <span class="flex items-center gap-0.5 shrink-0">
                                      <For each={eps().slice(0, 3)}>
                                        {(route) => (
                                          <span class={`inline-block font-mono text-[9px] font-medium px-1 py-0 rounded ring-1 ${
                                            ({
                                              get: "bg-emerald-500/15 text-emerald-400 ring-emerald-500/20",
                                              post: "bg-blue-500/15 text-blue-400 ring-blue-500/20",
                                              delete: "bg-red-500/15 text-red-400 ring-red-500/20",
                                              put: "bg-amber-500/15 text-amber-400 ring-amber-500/20",
                                              patch: "bg-violet-500/15 text-violet-400 ring-violet-500/20",
                                            } as Record<string, string>)[route.method.toLowerCase()] || "bg-gray-500/15 text-gray-400 ring-gray-500/20"
                                          }`}>{route.method.toUpperCase()}</span>
                                        )}
                                      </For>
                                      <Show when={eps().length > 3}>
                                        <span class="text-[9px] text-gray-500">+{eps().length - 3}</span>
                                      </Show>
                                    </span>
                                  </Show>
                                  {(edges()[defName]?.length || 0) > 0 && (
                                    <span class="text-[10px] text-gray-600 shrink-0">{edges()[defName].length}</span>
                                  )}
                                </button>
                                <Show when={def()?.extends}>
                                  <div class="px-3 pl-8 pb-1">
                                    <span class="text-xs text-gray-600">extends </span>
                                    <span
                                      class="text-xs text-purple-400 cursor-pointer hover:underline"
                                      onClick={() => props.selectEntity(def()!.extends!)}
                                    >{def()!.extends}</span>
                                  </div>
                                </Show>
                                <Show when={isExpanded() && def()}>
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
                                              onClick={() => props.selectEntity(prop.ref_name!)}
                                            >{prop.ref_name}</span>
                                          </Show>
                                          <Show when={prop.is_array && prop.items_ref}>
                                            <span
                                              class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400 cursor-pointer hover:underline"
                                              onClick={() => props.selectEntity(prop.items_ref!)}
                                            >[{prop.items_ref}]</span>
                                          </Show>
                                          <Show when={prop.is_array && !prop.items_ref}>
                                            <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400">[{prop.type}]</span>
                                          </Show>
                                          <Show when={prop.enum_values}>
                                            <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-pink-500/10 text-pink-400">enum</span>
                                          </Show>
                                          <Show when={!prop.ref_name && !prop.is_array && !prop.enum_values}>
                                            <span class={`px-1.5 py-0.5 rounded text-xs font-mono ${props.typeBadgeClass(prop.type, false, false)}`}>{prop.type}</span>
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
                    )}
                  </For>
                </Show>
                <Show when={virtualRoots().length > 0}>
                  <div class="mt-3 border-t border-gray-800/50 pt-2">
                    <button
                      class="w-full flex items-center gap-2 px-3 py-2 text-sm text-orange-300 hover:bg-orange-900/10 transition-colors"
                      onClick={() => setVrCollapsed(!vrCollapsed())}
                    >
                      <svg
                        class={`w-3 h-3 shrink-0 transition-transform ${vrCollapsed() ? "" : "rotate-90"}`}
                        fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"
                      >
                        <path stroke-linecap="round" stroke-linejoin="round" d="M9 5l7 7-7 7" />
                      </svg>
                      <span class="text-xs font-medium uppercase tracking-wider">Endpoints without schemas</span>
                      <span class="bg-orange-900/40 text-orange-300 px-2 py-0.5 rounded text-xs ml-auto">{virtualRoots().length}</span>
                    </button>
                    <Show when={!vrCollapsed()}>
                      <div class="space-y-1 px-1 pb-2">
                        <Show when={virtualRoots().length <= 10}>
                          <For each={virtualRoots()}>
                            {(vr) => (
                              <div class="rounded-md overflow-hidden border border-gray-800/50">
                                <div class="flex items-center gap-2 px-3 py-2 bg-orange-900/20 text-sm text-orange-300">
                                  <span class="font-mono text-xs">{vr.endpoint.method.toUpperCase()} {vr.endpoint.path}</span>
                                  <span class="bg-orange-900/40 text-orange-300 px-2 py-0.5 rounded text-xs ml-auto">{vr.shape}</span>
                                </div>
                              </div>
                            )}
                          </For>
                        </Show>
                        <Show when={virtualRoots().length > 10}>
                          {(() => {
                            const grouped = createMemo(() => {
                              const groups: Record<string, typeof virtualRoots extends () => (infer T)[] ? T[] : never> = {};
                              for (const vr of virtualRoots()) {
                                if (!groups[vr.shape]) groups[vr.shape] = [];
                                groups[vr.shape].push(vr);
                              }
                              return Object.entries(groups).sort(([a], [b]) => a.localeCompare(b));
                            });
                            return (
                              <For each={grouped()}>
                                {([shape, entries]) => (
                                  <div class="mb-2">
                                    <div class="text-[10px] font-medium text-gray-500 px-2 py-1">{shape} ({(entries as any[]).length})</div>
                                    <For each={entries as { endpoint: { method: string; path: string }; shape: string }[]}>
                                      {(vr) => (
                                        <div class="rounded-md overflow-hidden border border-gray-800/50">
                                          <div class="flex items-center gap-2 px-3 py-2 bg-orange-900/20 text-sm text-orange-300">
                                            <span class="font-mono text-xs">{vr.endpoint.method.toUpperCase()} {vr.endpoint.path}</span>
                                            <span class="bg-orange-900/40 text-orange-300 px-2 py-0.5 rounded text-xs ml-auto">{vr.shape}</span>
                                          </div>
                                        </div>
                                      )}
                                    </For>
                                  </div>
                                )}
                              </For>
                            );
                          })()}
                        </Show>
                      </div>
                    </Show>
                  </div>
                </Show>
              </div>
            </div>
          </div>

          {/* Right panel - Detail / Graph tabs */}
          <div class="flex-1 min-w-0 min-h-0 flex flex-col overflow-y-auto">
            {/* Empty state — no tabs */}
            <Show when={props.selectedEntities().size === 0}>
              <div class="rounded-xl bg-[#0a101d] border border-[#141b28] p-8 text-center">
                <p class="text-gray-600 text-sm">Select a definition to view its details.</p>
              </div>
            </Show>

            {/* Tabbed content — only when entity selected */}
            <Show when={props.selectedEntities().size > 0}>
              {/* Tab bar */}
              <div class="flex items-center gap-1 mb-3 shrink-0">
                <button
                  class={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
                    rightTab() === "details"
                      ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                      : "text-gray-400 hover:text-gray-200"
                  }`}
                  onClick={() => setRightTab("details")}
                >Details</button>
                <button
                  class={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
                    rightTab() === "graph"
                      ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                      : "text-gray-400 hover:text-gray-200"
                  }`}
                  onClick={() => setRightTab("graph")}
                >Graph</button>
              </div>

              {/* Details tab content */}
              <Show when={rightTab() === "details" && props.lastSelectedEntity() && props.definitions()[props.lastSelectedEntity()!]}>
                {(() => {
                  const defName = () => props.lastSelectedEntity()!;
                  const def = () => props.definitions()[defName()];
                  const eps = () => props.endpointsForDef(defName());
                  return (
                    <div class="rounded-xl bg-[#0a101d] border border-[#141b28] overflow-y-auto">
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
                              onClick={() => props.selectEntity(def()!.extends!)}
                            >{def()!.extends}</span>
                          </p>
                        </Show>
                      </div>

                      {/* Used by endpoints */}
                      <Show when={eps().length > 0}>
                        <div class="px-6 py-4 border-b border-[#141b28]">
                          <p class="text-xs font-medium text-gray-500 uppercase tracking-wider mb-3">Used by endpoints</p>
                          <div class="space-y-2">
                            <For each={eps()}>
                              {(route) => (
                                <div class="flex items-center gap-2 rounded border border-[#141b28] bg-[#0d1520] p-2">
                                  <MethodBadge method={route.method} />
                                  <span class="font-mono text-sm text-gray-400">{route.path}</span>
                                </div>
                              )}
                            </For>
                          </div>
                        </div>
                      </Show>

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
                                            onClick={() => props.selectEntity(prop.ref_name!)}
                                          >{prop.ref_name}</span>
                                        </Show>
                                        <Show when={prop.is_array && prop.items_ref}>
                                          <span
                                            class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400 cursor-pointer hover:underline"
                                            onClick={() => props.selectEntity(prop.items_ref!)}
                                          >[{prop.items_ref}]</span>
                                        </Show>
                                        <Show when={prop.is_array && !prop.items_ref}>
                                          <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-orange-500/10 text-orange-400">[{prop.type}]</span>
                                        </Show>
                                        <Show when={prop.enum_values}>
                                          <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-pink-500/10 text-pink-400">enum</span>
                                        </Show>
                                        <Show when={!prop.ref_name && !prop.is_array && !prop.enum_values}>
                                          <span class={`px-1.5 py-0.5 rounded text-xs font-mono ${props.typeBadgeClass(prop.type, false, false)}`}>
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
                    </div>
                  );
                })()}
              </Show>

              {/* Graph tab content */}
              <Show when={rightTab() === "graph"}>
                <div class="flex items-center justify-between mb-1 shrink-0">
                  <p class="text-xs font-medium text-gray-500 uppercase tracking-wider">Entity Graph</p>
                  <Show when={props.graphFocused() !== null}>
                    <button
                      class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded border border-gray-700/50"
                      onClick={() => props.collapseGraph()}
                    >Clear</button>
                  </Show>
                </div>
                <Show when={props.graphFocused() === null}>
                  <div class="flex-1 min-h-0 bg-[#070c17] border border-gray-800 rounded-lg flex items-center justify-center">
                    <p class="text-sm text-gray-600">Select a schema to view its ERD box</p>
                  </div>
                </Show>
                <Show when={props.graphFocused() !== null}>
                  {(() => {
                    // focusSet: selectedEntities filtered to known defs; fallback to graphFocused
                    const focusSet = createMemo<string[]>(() => {
                      const defs = props.definitions();
                      const sel = props.selectedEntities();
                      if (sel.size >= 1) {
                        const filtered = [...sel].filter(e => defs[e]);
                        if (filtered.length > 0) return filtered;
                      }
                      const gf = props.graphFocused();
                      return gf ? [gf] : [];
                    });
                    const focusMembership = createMemo(() => new Set(focusSet()));
                    const expandedSet = createMemo(() => props.graphExpanded());

                    // Derive immediate neighbors (one-hop) from relationship data, union over focusSet, exclude focals
                    const immediateNeighbors = createMemo(() => {
                      const focals = focusMembership();
                      const defs = props.definitions();
                      const neighbors = new Set<string>();

                      for (const f of focals) {
                        // Outbound: properties and extends of each focal
                        const focusedDef = defs[f];
                        if (focusedDef) {
                          if (focusedDef.extends && defs[focusedDef.extends] && !focals.has(focusedDef.extends)) {
                            neighbors.add(focusedDef.extends);
                          }
                          for (const prop of Object.values(focusedDef.properties)) {
                            const ref = prop.ref_name || prop.items_ref;
                            if (ref && defs[ref] && !focals.has(ref)) neighbors.add(ref);
                          }
                        }
                      }

                      // Inbound: other entities that reference any focal
                      for (const [entityName, def] of Object.entries(defs)) {
                        if (focals.has(entityName)) continue;
                        if (def.extends && focals.has(def.extends)) { neighbors.add(entityName); continue; }
                        for (const prop of Object.values(def.properties)) {
                          const ref = prop.ref_name || prop.items_ref;
                          if (ref && focals.has(ref)) { neighbors.add(entityName); break; }
                        }
                      }

                      return [...neighbors];
                    });

                    // Stubs = neighbors not in expandedSet; expanded = neighbors in expandedSet
                    const stubList = createMemo(() => {
                      const exp = expandedSet();
                      return immediateNeighbors().filter(n => !exp.has(n));
                    });
                    // All visible entities: focals + all neighbors (Set-deduped)
                    const visibleList = createMemo(() => [...new Set([...focusSet(), ...immediateNeighbors()])]);
                    const stubSet = createMemo(() => new Set(stubList()));

                    const boxWidth = 260;
                    const boxSpacing = 300;
                    const bandGap = 40;

                    const [hoveredEdgeId, setHoveredEdgeId] = createSignal<string | null>(null);

                    interface EdgeInfo {
                      id: string;
                      source: string;
                      sourceField: string;
                      sourceFieldIndex: number;
                      target: string;
                      cardinality: "1:1" | "1:N";
                      hasExtends: boolean;
                      required: boolean;
                      typeLabel: string;
                      refKind: "ref" | "items" | "extends";
                    }

                    // Compute edges between visible entities
                    const graphEdges = createMemo(() => {
                      const list = visibleList();
                      const visibleSet = new Set(list);
                      const defs = props.definitions();
                      const edgeList: EdgeInfo[] = [];

                      for (const entityName of list) {
                        const def = defs[entityName];
                        if (!def) continue;
                        const hasExtends = !!def.extends;

                        // extends pseudo-edge
                        if (def.extends && visibleSet.has(def.extends) && def.extends !== entityName) {
                          if (defs[def.extends]) {
                            edgeList.push({
                              id: `${entityName}::extends::${def.extends}`,
                              source: entityName,
                              sourceField: "extends",
                              sourceFieldIndex: 0,
                              target: def.extends,
                              cardinality: "1:1",
                              hasExtends: true,
                              required: false,
                              typeLabel: `extends ${def.extends}`,
                              refKind: "extends",
                            });
                          }
                        }

                        // Property edges
                        const entries = Object.entries(def.properties);
                        for (let fi = 0; fi < entries.length; fi++) {
                          const [fieldName, prop] = entries[fi];
                          const refName = prop.ref_name || prop.items_ref;
                          if (!refName) continue;
                          if (!visibleSet.has(refName)) continue;
                          if (refName === entityName) continue;
                          if (!defs[refName]) continue;
                          const isItems = !!prop.items_ref;
                          const typeLabel = isItems
                            ? `${refName}[]`
                            : (prop.ref_name ?? refName);
                          edgeList.push({
                            id: `${entityName}::${fieldName}::${refName}`,
                            source: entityName,
                            sourceField: fieldName,
                            sourceFieldIndex: fi,
                            target: refName,
                            cardinality: prop.is_array || !!prop.items_ref ? "1:N" : "1:1",
                            hasExtends,
                            required: !!prop.required,
                            typeLabel,
                            refKind: isItems ? "items" : "ref",
                          });
                        }
                      }

                      return edgeList;
                    });

                    // Compute DAG layout: positions + SVG dimensions
                    const dagLayout = createMemo(() =>
                      computeDagPositions(
                        visibleList(),
                        graphEdges(),
                        props.definitions(),
                        stubSet(),
                        boxSpacing,
                        bandGap,
                      )
                    );

                    const entityPositions = createMemo(() => dagLayout().positions);
                    const svgWidth = () => dagLayout().width;
                    const svgHeight = () => dagLayout().height;

                    // Box rects for edge routing obstruction checks
                    const boxRects = createMemo(() => {
                      const pos = entityPositions();
                      const defs = props.definitions();
                      const stubs = stubSet();
                      const rects: Record<string, { x: number; y: number; w: number; h: number }> = {};
                      for (const name of visibleList()) {
                        const p = pos[name];
                        if (!p) continue;
                        const def = defs[name];
                        let h = HEADER_HEIGHT;
                        if (def && !stubs.has(name)) {
                          const rowCt = Object.keys(def.properties).length + (def.extends ? 1 : 0) || 1;
                          h = HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
                        }
                        rects[name] = { x: p.x, y: p.y, w: boxWidth, h };
                      }
                      return rects;
                    });

                    // Click handlers
                    const handleStubClick = (entityName: string) => {
                      const next = new Set(props.graphExpanded());
                      next.add(entityName);
                      props.setGraphExpanded(next);
                      setHoveredEdgeId(null);
                    };

                    const handleExpandedNeighborClick = (entityName: string) => {
                      const next = new Set(props.graphExpanded());
                      next.delete(entityName);
                      props.setGraphExpanded(next);
                      setHoveredEdgeId(null);
                    };

                    // An edge is "focal-connected" when either endpoint is a focused entity.
                    // Focal-connected edges render a midpoint label without requiring hover.
                    const isFocalConnected = (edge: EdgeInfo) => {
                      const focals = focusMembership();
                      return focals.has(edge.source) || focals.has(edge.target);
                    };

                    // Pan/zoom interaction
                    let svgEl: SVGSVGElement | undefined;
                    let isPanning = false;
                    let dragStartX = 0;
                    let dragStartY = 0;
                    let panStartX = 0;
                    let panStartY = 0;
                    let dragOccurred = false;

                    onMount(() => {
                      if (!svgEl) return;
                      const wheelHandler = (e: WheelEvent) => {
                        e.preventDefault();
                        const scaleFactor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
                        props.setGraphZoom(z => Math.min(4, Math.max(0.1, z * scaleFactor)));
                      };
                      svgEl.addEventListener("wheel", wheelHandler, { passive: false });
                      onCleanup(() => svgEl?.removeEventListener("wheel", wheelHandler));
                    });

                    const handlePointerDown = (e: PointerEvent) => {
                      if ((e.target as Element).closest("[data-entity-box]")) return;
                      isPanning = true;
                      dragStartX = e.clientX;
                      dragStartY = e.clientY;
                      panStartX = props.graphPan().x;
                      panStartY = props.graphPan().y;
                      dragOccurred = false;
                      (e.currentTarget as Element).setPointerCapture(e.pointerId);
                    };

                    const handlePointerMove = (e: PointerEvent) => {
                      if (!isPanning) return;
                      const dx = e.clientX - dragStartX;
                      const dy = e.clientY - dragStartY;
                      if (Math.hypot(dx, dy) >= 4) {
                        dragOccurred = true;
                        props.setGraphPan({ x: panStartX + dx, y: panStartY + dy });
                      }
                    };

                    const handlePointerUp = (e: PointerEvent) => {
                      if (!isPanning) return;
                      isPanning = false;
                      (e.currentTarget as Element).releasePointerCapture(e.pointerId);
                      if (dragOccurred) {
                        e.stopPropagation();
                      }
                    };

                    const fitGraph = () => {
                      const positions = dagLayout().positions;
                      const keys = Object.keys(positions);
                      if (keys.length === 0) return;
                      if (!svgEl) return;
                      const viewportW = svgEl.clientWidth;
                      const viewportH = svgEl.clientHeight;
                      if (viewportW === 0 || viewportH === 0) return;
                      const defs = props.definitions();
                      const stubs = stubSet();
                      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
                      for (const name of keys) {
                        const p = positions[name];
                        const def = defs[name];
                        let h = HEADER_HEIGHT;
                        if (def && !stubs.has(name)) {
                          const rowCt = Object.keys(def.properties).length + (def.extends ? 1 : 0) || 1;
                          h = HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
                        }
                        if (p.x < minX) minX = p.x;
                        if (p.y < minY) minY = p.y;
                        if (p.x + boxWidth > maxX) maxX = p.x + boxWidth;
                        if (p.y + h > maxY) maxY = p.y + h;
                      }
                      const bboxW = maxX - minX;
                      const bboxH = maxY - minY;
                      if (bboxW <= 0 || bboxH <= 0) return;
                      const scale = Math.min(4, Math.max(0.1, Math.min(viewportW / bboxW, viewportH / bboxH) * 0.95));
                      const tx = (viewportW - bboxW * scale) / 2 - minX * scale;
                      const ty = (viewportH - bboxH * scale) / 2 - minY * scale;
                      batch(() => {
                        props.setGraphZoom(scale);
                        props.setGraphPan({ x: tx, y: ty });
                      });
                    };

                    return (
                      <div class="flex-1 min-h-0 flex flex-col overflow-hidden">
                        <div class="flex items-center justify-end gap-1 mb-1 shrink-0">
                          <button
                            class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded border border-gray-700/50"
                            onClick={() => props.setGraphExpanded(new Set(immediateNeighbors()))}
                          >Expand</button>
                          <button
                            class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded border border-gray-700/50"
                            onClick={() => props.setGraphExpanded(new Set())}
                          >Collapse</button>
                          <button
                            class="px-2 py-1 text-[10px] bg-gray-800/80 hover:bg-gray-700/80 text-gray-300 rounded border border-gray-700/50"
                            onClick={fitGraph}
                          >Fit</button>
                        </div>
                      <div class="flex-1 min-h-0 bg-[#070c17] border border-gray-800 rounded-lg overflow-hidden">
                        <svg
                          ref={el => { svgEl = el; }}
                          width="100%"
                          height="100%"
                          style="display:block;"
                          onPointerDown={handlePointerDown}
                          onPointerMove={handlePointerMove}
                          onPointerUp={handlePointerUp}
                        >
                          <g transform={`translate(${props.graphPan().x},${props.graphPan().y}) scale(${props.graphZoom()})`}>
                          {/* Relationship edges (rendered before boxes for correct z-order) */}
                          <g data-relationship-edges>
                            <For each={graphEdges()}>
                              {(edge) => {
                                // Use reactive getters so SolidJS tracks stubSet()/entityPositions() as dependencies
                                const sourcePos = () => entityPositions()[edge.source];
                                const targetPos = () => entityPositions()[edge.target];

                                // Source Y: field row position, or HEADER_HEIGHT/2 if collapsed
                                const sy = () => {
                                  const pos = sourcePos();
                                  if (!pos) return 0;
                                  if (stubSet().has(edge.source)) {
                                    return pos.y + HEADER_HEIGHT / 2;
                                  }
                                  const fieldRowOffset = edge.hasExtends ? edge.sourceFieldIndex + 1 : edge.sourceFieldIndex;
                                  const effectiveRow = edge.sourceField === "extends" ? 0 : fieldRowOffset;
                                  return pos.y + HEADER_HEIGHT + (effectiveRow + 0.5) * ROW_HEIGHT;
                                };
                                const sx = () => {
                                  const pos = sourcePos();
                                  return pos ? pos.x + boxWidth : 0;
                                };

                                // Target Y: header center (staggered for fan-in), or HEADER_HEIGHT/2 if collapsed
                                const ty = () => {
                                  const pos = targetPos();
                                  if (!pos) return 0;
                                  if (stubSet().has(edge.target)) {
                                    return pos.y + HEADER_HEIGHT / 2;
                                  }
                                  const allEdgesToTarget = graphEdges().filter(e => e.target === edge.target);
                                  const fanIdx = allEdgesToTarget.indexOf(edge);
                                  const fanCount = allEdgesToTarget.length;
                                  const stagger = fanCount > 1 ? (fanIdx - (fanCount - 1) / 2) * 6 : 0;
                                  return pos.y + HEADER_HEIGHT / 2 + stagger;
                                };
                                const tx = () => {
                                  const pos = targetPos();
                                  return pos ? pos.x : 0;
                                };

                                // Elbow connector: orthogonal routing that avoids box obstructions
                                // Returns the SVG path plus the midpoint of the longest horizontal segment
                                // (robust against backward/detour 5-segment elbows where simple average fails).
                                const route = (): { d: string; labelX: number; labelY: number } => {
                                  const _sx = sx(), _sy = sy(), _tx = tx(), _ty = ty();
                                  const rects = boxRects();
                                  const margin = 10;

                                  // Build path from a list of horizontal run x-values alternating with vertical jumps to y-values.
                                  // segments: [{x1,x2,y}] — pick the one with greatest |x2-x1| for label placement.
                                  const buildResult = (
                                    runs: { x1: number; x2: number; y: number }[],
                                    dPath: string,
                                  ): { d: string; labelX: number; labelY: number } => {
                                    let best = runs[0];
                                    let bestLen = Math.abs(best.x2 - best.x1);
                                    for (let i = 1; i < runs.length; i++) {
                                      const len = Math.abs(runs[i].x2 - runs[i].x1);
                                      if (len > bestLen) { best = runs[i]; bestLen = len; }
                                    }
                                    return {
                                      d: dPath,
                                      labelX: (best.x1 + best.x2) / 2,
                                      labelY: best.y,
                                    };
                                  };

                                  // Check if a vertical segment at x=vx from y1 to y2 intersects any box (excluding source & target)
                                  const verticalHitsBox = (vx: number, y1: number, y2: number): { x: number; y: number; w: number; h: number }[] => {
                                    const minY = Math.min(y1, y2);
                                    const maxY = Math.max(y1, y2);
                                    const hits: { x: number; y: number; w: number; h: number }[] = [];
                                    for (const [name, r] of Object.entries(rects)) {
                                      if (name === edge.source || name === edge.target) continue;
                                      if (vx >= r.x && vx <= r.x + r.w && maxY >= r.y && minY <= r.y + r.h) {
                                        hits.push(r);
                                      }
                                    }
                                    return hits;
                                  };

                                  // Backward edge: source exits right and loops back left to target
                                  if (_sx >= _tx) {
                                    // Find the rightmost edge among source and target boxes plus any boxes in between
                                    const srcRect = rects[edge.source];
                                    const tgtRect = rects[edge.target];
                                    let loopX = _sx + 20; // default: just past source right edge
                                    if (srcRect) loopX = Math.max(loopX, srcRect.x + srcRect.w + 20);
                                    if (tgtRect) loopX = Math.max(loopX, tgtRect.x + tgtRect.w + 20);

                                    // Also avoid any box whose right edge is near our loop path
                                    for (const [name, r] of Object.entries(rects)) {
                                      if (name === edge.source || name === edge.target) continue;
                                      const minY = Math.min(_sy, _ty);
                                      const maxY = Math.max(_sy, _ty);
                                      if (loopX >= r.x && loopX <= r.x + r.w + margin && maxY >= r.y && minY <= r.y + r.h) {
                                        loopX = r.x + r.w + 20;
                                      }
                                    }

                                    // Route: right from source, down/up to target Y, left to target
                                    return buildResult(
                                      [
                                        { x1: _sx, x2: loopX, y: _sy },
                                        { x1: loopX, x2: _tx, y: _ty },
                                      ],
                                      `M ${_sx} ${_sy} H ${loopX} V ${_ty} H ${_tx}`,
                                    );
                                  }

                                  // Forward edge: source is left of target
                                  const midX = (_sx + _tx) / 2;
                                  const hits = verticalHitsBox(midX, _sy, _ty);

                                  if (hits.length === 0) {
                                    // No obstruction: simple H-V-H elbow
                                    return buildResult(
                                      [
                                        { x1: _sx, x2: midX, y: _sy },
                                        { x1: midX, x2: _tx, y: _ty },
                                      ],
                                      `M ${_sx} ${_sy} H ${midX} V ${_ty} H ${_tx}`,
                                    );
                                  }

                                  // Try routing the vertical segment to the right of all blocking boxes
                                  const rightmostEdge = Math.max(...hits.map(h => h.x + h.w));
                                  const shiftedRight = rightmostEdge + margin;
                                  if (shiftedRight < _tx && verticalHitsBox(shiftedRight, _sy, _ty).length === 0) {
                                    return buildResult(
                                      [
                                        { x1: _sx, x2: shiftedRight, y: _sy },
                                        { x1: shiftedRight, x2: _tx, y: _ty },
                                      ],
                                      `M ${_sx} ${_sy} H ${shiftedRight} V ${_ty} H ${_tx}`,
                                    );
                                  }

                                  // Try routing to the left of all blocking boxes
                                  const leftmostEdge = Math.min(...hits.map(h => h.x));
                                  const shiftedLeft = leftmostEdge - margin;
                                  if (shiftedLeft > _sx && verticalHitsBox(shiftedLeft, _sy, _ty).length === 0) {
                                    return buildResult(
                                      [
                                        { x1: _sx, x2: shiftedLeft, y: _sy },
                                        { x1: shiftedLeft, x2: _tx, y: _ty },
                                      ],
                                      `M ${_sx} ${_sy} H ${shiftedLeft} V ${_ty} H ${_tx}`,
                                    );
                                  }

                                  // Route around: go right past all obstructions, then vertical, then left to target
                                  const minBlockY = Math.min(...hits.map(r => r.y));
                                  const maxBlockY = Math.max(...hits.map(r => r.y + r.h));

                                  // Decide whether to route above or below the obstruction
                                  const routeAboveY = minBlockY - margin;
                                  const routeBelowY = maxBlockY + margin;
                                  const distAbove = Math.abs(_sy - routeAboveY) + Math.abs(routeAboveY - _ty);
                                  const distBelow = Math.abs(_sy - routeBelowY) + Math.abs(routeBelowY - _ty);

                                  // Use the shorter detour
                                  const detourY = distAbove <= distBelow ? routeAboveY : routeBelowY;
                                  const detourX = rightmostEdge + margin;

                                  // H-V-H-V-H: exit source, jog vertically to clear the obstruction, then proceed to target
                                  const clearMidX = (_sx + Math.min(_tx, detourX)) / 2;
                                  return buildResult(
                                    [
                                      { x1: _sx, x2: clearMidX, y: _sy },
                                      { x1: clearMidX, x2: detourX, y: detourY },
                                      { x1: detourX, x2: _tx, y: _ty },
                                    ],
                                    `M ${_sx} ${_sy} H ${clearMidX} V ${detourY} H ${detourX} V ${_ty} H ${_tx}`,
                                  );
                                };

                                const routeInfo = createMemo(route);
                                const d = () => routeInfo().d;
                                const labelX = () => routeInfo().labelX;
                                const labelY = () => routeInfo().labelY;

                                const isHovered = () => hoveredEdgeId() === edge.id;
                                const hasPositions = () => sourcePos() && targetPos();
                                const focalConnected = () => isFocalConnected(edge);
                                const showLabel = () => isHovered() || focalConnected();

                                // Midpoint label text: "extends" for extends edges, else source field name.
                                const labelText = () =>
                                  edge.refKind === "extends" ? "extends" : edge.sourceField;

                                // Native SVG <title> content: "source.field -> target\ntype\nrequired: yes|no"
                                const tooltipText = () => {
                                  const head = edge.refKind === "extends"
                                    ? `${edge.source} extends ${edge.target}`
                                    : `${edge.source}.${edge.sourceField} -> ${edge.target}`;
                                  const typeLine = `type: ${edge.typeLabel}`;
                                  const reqLine = `required: ${edge.required ? "yes" : "no"}`;
                                  return `${head}\n${typeLine}\n${reqLine}`;
                                };

                                // Approximate label rect: 6px per char + 12px padding, 14px tall.
                                const labelWidth = () => labelText().length * 6 + 12;

                                return (
                                  <Show when={hasPositions()}>
                                    <g>
                                      {/* Wider invisible hit area — carries the native tooltip */}
                                      <path
                                        d={d()}
                                        fill="none"
                                        stroke="transparent"
                                        stroke-width={12}
                                        onMouseEnter={() => setHoveredEdgeId(edge.id)}
                                        onMouseLeave={() => setHoveredEdgeId(null)}
                                      >
                                        <title>{tooltipText()}</title>
                                      </path>
                                      {/* Visible path */}
                                      <path
                                        d={d()}
                                        fill="none"
                                        stroke={isHovered() ? "#60a5fa" : "#4b5563"}
                                        stroke-width={isHovered() ? 2 : 1}
                                        stroke-dasharray="none"
                                        data-relationship-line
                                        data-source-entity={edge.source}
                                        data-source-field={edge.sourceField}
                                        data-target-entity={edge.target}
                                        data-cardinality={edge.cardinality}
                                        data-edge-id={edge.id}
                                        onMouseEnter={() => setHoveredEdgeId(edge.id)}
                                        onMouseLeave={() => setHoveredEdgeId(null)}
                                      />
                                      {/* Cardinality label near source */}
                                      <text
                                        x={sx() + 6}
                                        y={sy() - 6}
                                        fill={isHovered() ? "#93c5fd" : "#6b7280"}
                                        font-size="10"
                                        font-family="ui-monospace, monospace"
                                      >1</text>
                                      {/* Cardinality label near target */}
                                      <text
                                        x={tx() - 14}
                                        y={ty() - 6}
                                        fill={isHovered() ? "#93c5fd" : "#6b7280"}
                                        font-size="10"
                                        font-family="ui-monospace, monospace"
                                        text-anchor="end"
                                      >{edge.cardinality === "1:N" ? "N" : "1"}</text>
                                      {/* Arrow marker at target end */}
                                      <polygon
                                        points={`${tx()},${ty()} ${tx() - 6},${ty() - 3} ${tx() - 6},${ty() + 3}`}
                                        fill={isHovered() ? "#60a5fa" : "#4b5563"}
                                      />
                                      {/* Midpoint field-name label (focal-connected edges always; non-focal edges only on hover) */}
                                      <Show when={showLabel()}>
                                        <g pointer-events="none" data-edge-label-for={edge.id}>
                                          <rect
                                            x={labelX() - labelWidth() / 2}
                                            y={labelY() - 8}
                                            width={labelWidth()}
                                            height={14}
                                            rx={3}
                                            ry={3}
                                            fill="#070c17"
                                            fill-opacity={0.9}
                                            stroke={isHovered() ? "#60a5fa" : "#374151"}
                                            stroke-width={0.5}
                                          />
                                          <text
                                            x={labelX()}
                                            y={labelY() + 3}
                                            fill={isHovered() ? "#93c5fd" : "#9ca3af"}
                                            font-size="10"
                                            font-family="ui-monospace, monospace"
                                            text-anchor="middle"
                                          >{labelText()}</text>
                                        </g>
                                      </Show>
                                    </g>
                                  </Show>
                                );
                              }}
                            </For>
                          </g>
                          {/* Entity boxes */}
                          <For each={visibleList()}>
                            {(entityName, idx) => {
                              const def = () => props.definitions()[entityName];
                              const xPos = () => entityPositions()[entityName]?.x ?? 0;
                              const yPos = () => entityPositions()[entityName]?.y ?? 0;

                              const isFocused = () => focusMembership().has(entityName);
                              const isStub = () => stubSet().has(entityName);
                              const isExpandedNeighbor = () => !isFocused() && !isStub();

                              // Check if any hovered edge involves this entity
                              const isHighlighted = () => {
                                const hId = hoveredEdgeId();
                                if (!hId) return false;
                                const edge = graphEdges().find(e => e.id === hId);
                                if (!edge) return false;
                                return edge.source === entityName || edge.target === entityName;
                              };

                              const boxTotalHeight = () => {
                                if (isStub()) return HEADER_HEIGHT;
                                const d = def();
                                if (!d) return HEADER_HEIGHT;
                                const rowCt = Object.keys(d.properties).length + (d.extends ? 1 : 0) || 1;
                                return HEADER_HEIGHT + Math.min(rowCt, 10) * ROW_HEIGHT;
                              };

                              return (
                                <Show when={def()}>
                                  <g
                                    data-entity-highlighted={isHighlighted() ? "true" : undefined}
                                    {...(isStub() ? { "data-neighbor-stub": "true", "data-entity-name": entityName } : {})}
                                  >
                                    <Show when={isHighlighted()}>
                                      <rect
                                        x={xPos() - 2}
                                        y={yPos() - 2}
                                        width={boxWidth + 4}
                                        height={boxTotalHeight() + 4}
                                        rx={6}
                                        ry={6}
                                        fill="none"
                                        stroke="#3b82f6"
                                        stroke-width={1.5}
                                        stroke-opacity={0.5}
                                      />
                                    </Show>
                                    <EntityBox
                                      name={entityName}
                                      properties={def()?.properties || {}}
                                      extends={def()?.extends}
                                      x={xPos()}
                                      y={yPos()}
                                      width={boxWidth}
                                      collapsed={isStub()}
                                      onHeaderClick={
                                        isStub()
                                          ? () => handleStubClick(entityName)
                                          : isExpandedNeighbor()
                                          ? () => handleExpandedNeighborClick(entityName)
                                          : undefined
                                      }
                                      onSelectRef={(refName) => props.selectEntity(refName)}
                                    />
                                  </g>
                                </Show>
                              );
                            }}
                          </For>
                          </g>
                        </svg>
                      </div>
                      </div>
                    );
                  })()}
                </Show>
              </Show>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}

function RecipeConfigStep(props: {
  recipeSharedPools: Accessor<Record<string, { is_shared: boolean; pool_size: number }>>;
  setRecipeSharedPools: Setter<Record<string, { is_shared: boolean; pool_size: number }>>;
  recipeQuantityConfigs: Accessor<Record<string, { min: number; max: number }>>;
  setRecipeQuantityConfigs: Setter<Record<string, { min: number; max: number }>>;
  recipeFakerRules: Accessor<Record<string, string>>;
  setRecipeFakerRules: Setter<Record<string, string>>;
  recipeRules: Accessor<Rule[]>;
  setRecipeRules: Setter<Rule[]>;
  entityGraph: Accessor<any>;
  configSearch: Accessor<string>;
  setConfigSearch: Setter<string>;
  configShowNonDefault: Accessor<boolean>;
  setConfigShowNonDefault: Setter<boolean>;
}) {
  const hasPools = () => Object.keys(props.recipeSharedPools()).length > 0;
  const hasConfigs = () => Object.keys(props.recipeQuantityConfigs()).length > 0;
  const hasRules = () => Object.keys(props.recipeFakerRules()).length > 0;

  // Invert graph.roots: defName→endpoints[] into endpointLabel→defNames[]
  // Classify definitions into single-endpoint, shared, or nested buckets
  type EndpointBucket = { label: string; defs: string[] };
  const endpointBuckets = createMemo((): EndpointBucket[] => {
    const graph = props.entityGraph();
    const roots: Record<string, { method: string; path: string }[]> = graph?.roots || {};
    const epToDefs: Record<string, Set<string>> = {};
    const defToEps: Record<string, string[]> = {};

    for (const [defName, eps] of Object.entries(roots)) {
      for (const ep of eps) {
        const label = `${ep.method.toUpperCase()} ${ep.path}`;
        if (!epToDefs[label]) epToDefs[label] = new Set();
        epToDefs[label].add(defName);
        if (!defToEps[defName]) defToEps[defName] = [];
        defToEps[defName].push(label);
      }
    }

    // Collect all known definition names across pools, configs, faker rules
    const allDefs = new Set<string>();
    for (const name of Object.keys(props.recipeSharedPools())) allDefs.add(name);
    for (const key of Object.keys(props.recipeQuantityConfigs())) {
      const dot = key.indexOf(".");
      allDefs.add(dot >= 0 ? key.slice(0, dot) : key);
    }
    for (const key of Object.keys(props.recipeFakerRules())) {
      const dot = key.indexOf(".");
      allDefs.add(dot >= 0 ? key.slice(0, dot) : key);
    }

    // Sort endpoint labels by path then method
    const sortedLabels = Object.keys(epToDefs).sort((a, b) => {
      const [am, ...ap] = a.split(" ");
      const [bm, ...bp] = b.split(" ");
      const pathCmp = ap.join(" ").localeCompare(bp.join(" "));
      return pathCmp !== 0 ? pathCmp : am.localeCompare(bm);
    });

    const buckets: EndpointBucket[] = [];
    const assignedSingle = new Set<string>();
    const sharedDefs = new Set<string>();

    // Single-endpoint roots go under their endpoint
    for (const label of sortedLabels) {
      const defs: string[] = [];
      for (const d of epToDefs[label]) {
        if ((defToEps[d] || []).length === 1) {
          defs.push(d);
          assignedSingle.add(d);
        } else {
          sharedDefs.add(d);
        }
      }
      if (defs.length > 0) {
        buckets.push({ label, defs: defs.sort() });
      }
    }

    // Shared bucket: definitions that appear in 2+ endpoints
    const sharedArr = [...sharedDefs].sort();
    if (sharedArr.length > 0) {
      buckets.push({ label: "Shared", defs: sharedArr });
    }

    // Nested bucket: definitions not in roots at all
    const nestedDefs = [...allDefs].filter(d => !assignedSingle.has(d) && !sharedDefs.has(d)).sort();
    if (nestedDefs.length > 0) {
      buckets.push({ label: "Nested", defs: nestedDefs });
    }

    return buckets;
  });

  // Map from defName to its bucket label (for search matching)
  const defToBucket = createMemo((): Record<string, string> => {
    const map: Record<string, string> = {};
    for (const bucket of endpointBuckets()) {
      for (const d of bucket.defs) {
        map[d] = bucket.label;
      }
    }
    return map;
  });

  // Virtual buckets: unmapped endpoints with non-$ref response shapes
  type VirtualBucket = { label: string; shape_label: string };
  const virtualBuckets = createMemo((): VirtualBucket[] => {
    const graph = props.entityGraph();
    const vrs: { endpoint: { method: string; path: string }; shape: string }[] = graph?.virtual_roots || [];
    const existingLabels = new Set(endpointBuckets().map(b => b.label));
    return vrs
      .map(vr => ({ label: `${vr.endpoint.method.toUpperCase()} ${vr.endpoint.path}`, shape_label: vr.shape }))
      .filter(vb => !existingLabels.has(vb.label));
  });

  const hasAnything = () => hasPools() || hasConfigs() || hasRules() || virtualBuckets().length > 0;

  const filteredVirtualBuckets = createMemo((): VirtualBucket[] => {
    const q = props.configSearch().toLowerCase();
    if (!q) return virtualBuckets();
    return virtualBuckets().filter(vb =>
      vb.label.toLowerCase().includes(q) || vb.shape_label.toLowerCase().includes(q)
    );
  });

  // Group array quantity configs by entity (part before the dot)
  const groupedConfigs = () => {
    const groups: Record<string, { key: string; config: { min: number; max: number } }[]> = {};
    for (const [key, config] of Object.entries(props.recipeQuantityConfigs())) {
      const dot = key.indexOf(".");
      const entity = dot >= 0 ? key.slice(0, dot) : key;
      if (!groups[entity]) groups[entity] = [];
      groups[entity].push({ key, config });
    }
    return groups;
  };

  // Check if search query matches an endpoint bucket label
  const bucketMatchesSearch = (bucketLabel: string, q: string): boolean => {
    if (!q) return true;
    return bucketLabel.toLowerCase().includes(q);
  };

  // Filter by search — also match endpoint paths
  const filteredPools = () => {
    const q = props.configSearch().toLowerCase();
    const entries = Object.entries(props.recipeSharedPools());
    const filtered = q ? entries.filter(([e]) => {
      const bucket = defToBucket()[e] || "";
      return e.toLowerCase().includes(q) || bucket.toLowerCase().includes(q);
    }) : entries;
    if (props.configShowNonDefault()) return filtered.filter(([_, c]) => !c.is_shared || c.pool_size !== 10);
    return filtered;
  };

  // Group filtered pools by endpoint bucket
  const poolsByBucket = () => {
    const pools = filteredPools();
    const poolMap = new Map(pools);
    const buckets = endpointBuckets();
    const result: { label: string; pools: [string, { is_shared: boolean; pool_size: number }][] }[] = [];
    for (const bucket of buckets) {
      const bucketPools: [string, { is_shared: boolean; pool_size: number }][] = [];
      for (const def of bucket.defs) {
        if (poolMap.has(def)) {
          bucketPools.push([def, poolMap.get(def)!]);
        }
      }
      if (bucketPools.length > 0) {
        result.push({ label: bucket.label, pools: bucketPools });
      }
    }
    return result;
  };

  const filteredConfigGroups = () => {
    const q = props.configSearch().toLowerCase();
    const groups = groupedConfigs();
    const result: Record<string, { key: string; config: { min: number; max: number } }[]> = {};
    for (const [entity, items] of Object.entries(groups)) {
      const bucket = defToBucket()[entity] || "";
      const matchesBucket = bucketMatchesSearch(bucket, q);
      const matching = items.filter(i => !q || matchesBucket || i.key.toLowerCase().includes(q) || entity.toLowerCase().includes(q));
      const visible = props.configShowNonDefault() ? matching.filter(i => i.config.min !== 1 || i.config.max !== 3) : matching;
      if (visible.length > 0) result[entity] = visible;
    }
    return result;
  };

  // Group filtered config groups by endpoint bucket
  const configsByBucket = () => {
    const configs = filteredConfigGroups();
    const buckets = endpointBuckets();
    const result: { label: string; entities: [string, { key: string; config: { min: number; max: number } }[]][] }[] = [];
    for (const bucket of buckets) {
      const entities: [string, { key: string; config: { min: number; max: number } }[]][] = [];
      for (const def of bucket.defs) {
        if (configs[def]) {
          entities.push([def, configs[def]]);
        }
      }
      if (entities.length > 0) {
        result.push({ label: bucket.label, entities });
      }
    }
    return result;
  };

  const FAKER_STRATEGIES = ["auto", "word", "name", "email", "phone", "url", "sentence", "paragraph", "uuid", "date", "integer", "float", "boolean"];

  const groupedRules = () => {
    const groups: Record<string, { key: string; strategy: string; propType: string; format: string | null }[]> = {};
    const graph = props.entityGraph();
    const scalarMap: Record<string, { prop_type: string; format: string | null }> = {};
    for (const sp of graph?.scalar_properties || []) {
      scalarMap[`${sp.def_name}.${sp.prop_name}`] = { prop_type: sp.prop_type, format: sp.format };
    }
    for (const [key, strategy] of Object.entries(props.recipeFakerRules())) {
      const dot = key.indexOf(".");
      const entity = dot >= 0 ? key.slice(0, dot) : key;
      if (!groups[entity]) groups[entity] = [];
      const meta = scalarMap[key] || { prop_type: "string", format: null };
      groups[entity].push({ key, strategy, propType: meta.prop_type, format: meta.format });
    }
    return groups;
  };

  const filteredRuleGroups = () => {
    const q = props.configSearch().toLowerCase();
    const groups = groupedRules();
    const result: typeof groups = {};
    for (const [entity, items] of Object.entries(groups)) {
      const bucket = defToBucket()[entity] || "";
      const matchesBucket = bucketMatchesSearch(bucket, q);
      const matching = items.filter(i => !q || matchesBucket || i.key.toLowerCase().includes(q) || entity.toLowerCase().includes(q));
      const visible = props.configShowNonDefault() ? matching.filter(i => i.strategy !== "auto") : matching;
      if (visible.length > 0) result[entity] = visible;
    }
    return result;
  };

  // Group filtered rule groups by endpoint bucket
  const rulesByBucket = () => {
    const rules = filteredRuleGroups();
    const buckets = endpointBuckets();
    const result: { label: string; entities: [string, { key: string; strategy: string; propType: string; format: string | null }[]][] }[] = [];
    for (const bucket of buckets) {
      const entities: [string, { key: string; strategy: string; propType: string; format: string | null }[]][] = [];
      for (const def of bucket.defs) {
        if (rules[def]) {
          entities.push([def, rules[def]]);
        }
      }
      if (entities.length > 0) {
        result.push({ label: bucket.label, entities });
      }
    }
    return result;
  };

  // Indexed lookups: bucket label → data for O(1) access in endpoint-first rendering
  const poolsByLabel = createMemo((): Record<string, [string, { is_shared: boolean; pool_size: number }][]> => {
    const map: Record<string, [string, { is_shared: boolean; pool_size: number }][]> = {};
    for (const b of poolsByBucket()) map[b.label] = b.pools;
    return map;
  });

  const configsByLabel = createMemo((): Record<string, [string, { key: string; config: { min: number; max: number } }[]][]> => {
    const map: Record<string, [string, { key: string; config: { min: number; max: number } }[]][]> = {};
    for (const b of configsByBucket()) map[b.label] = b.entities;
    return map;
  });

  const rulesByLabel = createMemo((): Record<string, [string, { key: string; strategy: string; propType: string; format: string | null }[]][]> => {
    const map: Record<string, [string, { key: string; strategy: string; propType: string; format: string | null }[]][]> = {};
    for (const b of rulesByBucket()) map[b.label] = b.entities;
    return map;
  });

  // Bucket constraint rules (recipeRules) by endpoint, preserving global indices for mutations
  const constraintsByBucket = createMemo((): { label: string; rules: { rule: Rule; globalIndex: number }[] }[] => {
    const allRules = props.recipeRules();
    const dtb = defToBucket();
    const buckets = endpointBuckets();
    // Build a map: bucket label → entries
    const bucketMap: Record<string, { rule: Rule; globalIndex: number }[]> = {};
    for (let i = 0; i < allRules.length; i++) {
      const rule = allRules[i];
      const defName = rule.kind === "compare"
        ? (rule as CompareRule).left.split(".")[0]
        : (rule as Exclude<Rule, CompareRule>).field.split(".")[0];
      const bucketLabel = dtb[defName] || "Shared";
      if (!bucketMap[bucketLabel]) bucketMap[bucketLabel] = [];
      bucketMap[bucketLabel].push({ rule, globalIndex: i });
    }
    const result: { label: string; rules: { rule: Rule; globalIndex: number }[] }[] = [];
    for (const bucket of buckets) {
      if (bucketMap[bucket.label]) {
        result.push({ label: bucket.label, rules: bucketMap[bucket.label] });
      }
    }
    // Include rules for labels not matched to any bucket (fallback to Shared)
    if (bucketMap["Shared"] && !result.some(r => r.label === "Shared")) {
      result.push({ label: "Shared", rules: bucketMap["Shared"] });
    }
    return result;
  });

  const constraintsByLabel = createMemo((): Record<string, { rule: Rule; globalIndex: number }[]> => {
    const map: Record<string, { rule: Rule; globalIndex: number }[]> = {};
    for (const b of constraintsByBucket()) map[b.label] = b.rules;
    return map;
  });

  // Combined list of endpoint labels that have any visible data
  const activeEndpoints = createMemo((): string[] => {
    const pools = poolsByLabel();
    const configs = configsByLabel();
    const rules = rulesByLabel();
    const constraints = constraintsByLabel();
    return endpointBuckets()
      .map(b => b.label)
      .filter(label => (pools[label]?.length ?? 0) > 0 || (configs[label]?.length ?? 0) > 0 || (rules[label]?.length ?? 0) > 0 || (constraints[label]?.length ?? 0) > 0);
  });

  // Collapsed state for endpoint and entity groups
  const [collapsedGroups, setCollapsedGroups] = createSignal<Set<string>>(new Set());
  const toggleGroup = (key: string) => {
    const s = new Set(collapsedGroups());
    if (s.has(key)) s.delete(key); else s.add(key);
    setCollapsedGroups(s);
  };

  // Reset collapse state when entityGraph changes
  createEffect(() => {
    props.entityGraph();
    setCollapsedGroups(new Set<string>());
  });

  return (
    <div>
      <div class="flex items-center justify-between mb-3">
        <div>
          <h3 class="text-lg font-semibold">Configure Data Generation</h3>
          <p class="text-sm text-gray-500">
            {Object.keys(props.recipeSharedPools()).length} shared pools · {Object.keys(props.recipeQuantityConfigs()).length} array properties · {Object.keys(props.recipeFakerRules()).length} field rules · {props.recipeRules().length} constraint rules
          </p>
        </div>
        <Show when={hasAnything()}>
          <div class="flex items-center gap-3">
            <label class="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer">
              <input type="checkbox" checked={props.configShowNonDefault()} onChange={(e) => props.setConfigShowNonDefault(e.target.checked)} class="accent-blue-500 rounded" />
              Changed only
            </label>
          </div>
        </Show>
      </div>

      <Show when={hasAnything()}>
        <input
          type="text"
          placeholder="Search endpoints, pools, properties, and rules..."
          value={props.configSearch()}
          onInput={(e) => props.setConfigSearch(e.currentTarget.value)}
          class="w-full bg-[#070c17] border border-gray-800 rounded-lg px-3 py-2 text-sm text-gray-100 placeholder-gray-600 focus:outline-none focus:border-gray-700 focus:ring-1 focus:ring-gray-700 mb-4"
        />
      </Show>

      {/* Bulk controls — apply across all endpoints */}
      <Show when={hasAnything()}>
        <div class="flex flex-wrap items-center gap-4 mb-4 px-3 py-2 bg-gray-900/50 rounded-lg border border-gray-800/50">
          <Show when={hasPools()}>
            <div class="flex items-center gap-2">
              <span class="text-[10px] text-gray-500 font-medium uppercase tracking-wider">All pools</span>
              <input
                type="number" min="1" max="100" placeholder="n"
                class="w-14 bg-[#070c17] border border-gray-800 rounded px-1.5 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                onChange={(e) => {
                  const val = parseInt(e.target.value);
                  if (!val || val < 1) return;
                  const pools = { ...props.recipeSharedPools() };
                  for (const key of Object.keys(pools)) {
                    pools[key] = { ...pools[key], pool_size: val };
                  }
                  props.setRecipeSharedPools(pools);
                  e.target.value = "";
                }}
              />
            </div>
          </Show>
          <Show when={hasConfigs()}>
            <div class="flex items-center gap-2">
              <span class="text-[10px] text-gray-500 font-medium uppercase tracking-wider">All arrays</span>
              <input
                type="number" min="0" max="50" placeholder="min"
                class="w-12 bg-[#070c17] border border-gray-800 rounded px-1.5 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                onChange={(e) => {
                  const val = parseInt(e.target.value);
                  if (isNaN(val) || val < 0) return;
                  const configs = { ...props.recipeQuantityConfigs() };
                  for (const key of Object.keys(configs)) {
                    configs[key] = { ...configs[key], min: val };
                  }
                  props.setRecipeQuantityConfigs(configs);
                  e.target.value = "";
                }}
              />
              <span class="text-[10px] text-gray-600">–</span>
              <input
                type="number" min="1" max="50" placeholder="max"
                class="w-12 bg-[#070c17] border border-gray-800 rounded px-1.5 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                onChange={(e) => {
                  const val = parseInt(e.target.value);
                  if (!val || val < 1) return;
                  const configs = { ...props.recipeQuantityConfigs() };
                  for (const key of Object.keys(configs)) {
                    configs[key] = { ...configs[key], max: val };
                  }
                  props.setRecipeQuantityConfigs(configs);
                  e.target.value = "";
                }}
              />
            </div>
          </Show>
          <Show when={hasRules()}>
            <div class="flex items-center gap-2">
              <span class="text-[10px] text-gray-500 font-medium uppercase tracking-wider">All rules</span>
              <select
                class="bg-[#070c17] border border-gray-800 rounded px-1.5 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                onChange={(e) => {
                  const val = e.target.value;
                  if (!val) return;
                  const rules = { ...props.recipeFakerRules() };
                  for (const key of Object.keys(rules)) {
                    rules[key] = val;
                  }
                  props.setRecipeFakerRules(rules);
                  e.target.value = "";
                }}
              >
                <option value="">--</option>
                <For each={FAKER_STRATEGIES}>
                  {(s) => <option value={s}>{s}</option>}
                </For>
              </select>
            </div>
          </Show>
        </div>
      </Show>

      {/* Endpoint-first groups */}
      <div class="space-y-3">
        <For each={activeEndpoints()}>
          {(label) => {
            const epPools = () => poolsByLabel()[label] || [];
            const epConfigs = () => configsByLabel()[label] || [];
            const epRules = () => rulesByLabel()[label] || [];
            const epConstraints = () => constraintsByLabel()[label] || [];
            const epBucket = () => endpointBuckets().find(b => b.label === label);
            const epKey = () => `ep:${label}`;

            return (
              <div class="rounded-md overflow-hidden border border-gray-800/50">
                {/* Endpoint header */}
                <button
                  data-testid="endpoint-group-header"
                  class="w-full flex items-center gap-2 px-3 py-2 bg-blue-900/20 hover:bg-blue-900/30 text-sm text-blue-300 transition-colors"
                  onClick={() => toggleGroup(epKey())}
                >
                  <span class={`text-[10px] text-blue-400 transition-transform ${collapsedGroups().has(epKey()) ? "" : "rotate-90"}`}>&#9654;</span>
                  <span class="font-mono text-xs">{label}</span>
                  <span class="text-xs text-gray-600 ml-auto">
                    {epPools().length > 0 ? `${epPools().length} pools` : ""}
                    {epPools().length > 0 && (epConfigs().length > 0 || epRules().length > 0 || epConstraints().length > 0) ? " · " : ""}
                    {epConfigs().length > 0 ? `${epConfigs().reduce((n, [, items]) => n + items.length, 0)} arrays` : ""}
                    {epConfigs().length > 0 && (epRules().length > 0 || epConstraints().length > 0) ? " · " : ""}
                    {epRules().length > 0 ? `${epRules().reduce((n, [, items]) => n + items.length, 0)} rules` : ""}
                    {epRules().length > 0 && epConstraints().length > 0 ? " · " : ""}
                    {epConstraints().length > 0 ? `${epConstraints().length} constraints` : ""}
                  </span>
                </button>

                <Show when={!collapsedGroups().has(epKey())}>
                  <div class="px-2 py-2 space-y-2">

                    {/* Pools sub-section */}
                    <Show when={epPools().length > 0}>
                      <div>
                        <div class="px-2 py-1 text-[10px] text-gray-500 font-medium uppercase tracking-wider">Shared Pools</div>
                        <div class="space-y-1">
                          <For each={epPools()}>
                            {([entity, config]) => (
                              <div class="flex items-center gap-3 px-3 py-2 bg-gray-800/50 rounded-md ml-2">
                                <input type="checkbox" checked={config.is_shared}
                                  class="accent-blue-500 rounded"
                                  onChange={(e) => {
                                    const pools = { ...props.recipeSharedPools() };
                                    pools[entity] = { ...pools[entity], is_shared: e.target.checked };
                                    props.setRecipeSharedPools(pools);
                                  }} />
                                <span class="text-sm text-gray-200 flex-1 truncate">{entity}</span>
                                <input type="number" min="1" max="100" value={config.pool_size}
                                  class="w-16 bg-[#070c17] border border-gray-800 rounded-md px-2 py-1 text-sm text-gray-100 text-center focus:outline-none focus:border-gray-700"
                                  onInput={(e) => {
                                    const pools = { ...props.recipeSharedPools() };
                                    pools[entity] = { ...pools[entity], pool_size: parseInt(e.target.value) || 10 };
                                    props.setRecipeSharedPools(pools);
                                  }} />
                                <span class="text-xs text-gray-600 w-14">instances</span>
                              </div>
                            )}
                          </For>
                        </div>
                      </div>
                    </Show>

                    {/* Arrays sub-section */}
                    <Show when={epConfigs().length > 0}>
                      <div>
                        <div class="px-2 py-1 text-[10px] text-gray-500 font-medium uppercase tracking-wider">Array Quantities</div>
                        <div class="space-y-1 ml-2">
                          <For each={epConfigs()}>
                            {([entity, items]) => {
                              const arrKey = () => `ep:${label}:arrays:${entity}`;
                              return (
                                <div class="rounded-md overflow-hidden">
                                  <button
                                    class="w-full flex items-center gap-2 px-3 py-2 bg-gray-800/70 hover:bg-gray-800 text-sm text-gray-200 transition-colors"
                                    onClick={() => toggleGroup(arrKey())}
                                  >
                                    <span class={`text-[10px] text-gray-500 transition-transform ${collapsedGroups().has(arrKey()) ? "" : "rotate-90"}`}>&#9654;</span>
                                    <span class="font-medium">{entity}</span>
                                    <span class="text-xs text-gray-600 ml-auto">{items.length} {items.length === 1 ? "property" : "properties"}</span>
                                  </button>
                                  <Show when={!collapsedGroups().has(arrKey())}>
                                    <div class="bg-gray-900/30 border-l-2 border-gray-800 ml-3">
                                      <For each={items}>
                                        {(item) => {
                                          const propName = item.key.includes(".") ? item.key.split(".").slice(1).join(".") : item.key;
                                          return (
                                            <div class="flex items-center gap-3 px-3 py-1.5">
                                              <span class="font-mono text-xs text-gray-400 flex-1 truncate">.{propName}</span>
                                              <input type="number" min="0" max="50" value={item.config.min}
                                                class="w-14 bg-[#070c17] border border-gray-800 rounded px-2 py-0.5 text-xs text-gray-100 text-center focus:outline-none focus:border-gray-700"
                                                onInput={(e) => {
                                                  const configs = { ...props.recipeQuantityConfigs() };
                                                  configs[item.key] = { ...configs[item.key], min: parseInt(e.target.value) || 0 };
                                                  props.setRecipeQuantityConfigs(configs);
                                                }} />
                                              <span class="text-gray-600 text-xs">–</span>
                                              <input type="number" min="1" max="50" value={item.config.max}
                                                class="w-14 bg-[#070c17] border border-gray-800 rounded px-2 py-0.5 text-xs text-gray-100 text-center focus:outline-none focus:border-gray-700"
                                                onInput={(e) => {
                                                  const configs = { ...props.recipeQuantityConfigs() };
                                                  configs[item.key] = { ...configs[item.key], max: parseInt(e.target.value) || 3 };
                                                  props.setRecipeQuantityConfigs(configs);
                                                }} />
                                              <span class="text-[10px] text-gray-600 w-10">items</span>
                                            </div>
                                          );
                                        }}
                                      </For>
                                    </div>
                                  </Show>
                                </div>
                              );
                            }}
                          </For>
                        </div>
                      </div>
                    </Show>

                    {/* Rules sub-section */}
                    <Show when={epRules().length > 0}>
                      <div>
                        <div class="px-2 py-1 text-[10px] text-gray-500 font-medium uppercase tracking-wider">Field Rules</div>
                        <div class="space-y-1 ml-2">
                          <For each={epRules()}>
                            {([entity, items]) => {
                              const ruleKey = () => `ep:${label}:rules:${entity}`;
                              return (
                                <div class="rounded-md overflow-hidden">
                                  <button
                                    class="w-full flex items-center gap-2 px-3 py-2 bg-gray-800/70 hover:bg-gray-800 text-sm text-gray-200 transition-colors"
                                    onClick={() => toggleGroup(ruleKey())}
                                  >
                                    <span class={`text-[10px] text-gray-500 transition-transform ${collapsedGroups().has(ruleKey()) ? "" : "rotate-90"}`}>&#9654;</span>
                                    <span class="font-medium">{entity}</span>
                                    <span class="text-xs text-gray-600 ml-auto">{items.length} {items.length === 1 ? "field" : "fields"}</span>
                                  </button>
                                  <Show when={!collapsedGroups().has(ruleKey())}>
                                    <div class="bg-gray-900/30 border-l-2 border-gray-800 ml-3">
                                      <For each={items}>
                                        {(item) => {
                                          const propName = item.key.includes(".") ? item.key.split(".").slice(1).join(".") : item.key;
                                          return (
                                            <div class="flex items-center gap-3 px-3 py-1.5">
                                              <span class="font-mono text-xs text-gray-400 flex-1 truncate">.{propName}</span>
                                              <span class="text-[10px] px-1.5 py-0.5 rounded bg-gray-800 text-gray-500">{item.propType}{item.format ? `/${item.format}` : ""}</span>
                                              <select
                                                value={item.strategy}
                                                class="bg-[#070c17] border border-gray-800 rounded-md px-2 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                                                onChange={(e) => {
                                                  const rules = { ...props.recipeFakerRules() };
                                                  rules[item.key] = e.target.value;
                                                  props.setRecipeFakerRules(rules);
                                                }}
                                              >
                                                <For each={FAKER_STRATEGIES}>
                                                  {(s) => <option value={s}>{s}</option>}
                                                </For>
                                              </select>
                                            </div>
                                          );
                                        }}
                                      </For>
                                    </div>
                                  </Show>
                                </div>
                              );
                            }}
                          </For>
                        </div>
                      </div>
                    </Show>

                    {/* Constraint Rules sub-section */}
                    <ConstraintRulesEditor
                      rules={epConstraints}
                      recipeRules={props.recipeRules}
                      setRecipeRules={props.setRecipeRules}
                      entityGraph={props.entityGraph}
                      scopedDefs={epBucket()?.defs}
                    />

                  </div>
                </Show>
              </div>
            );
          }}
        </For>

        {/* Unmapped endpoints — display-only, non-$ref response shapes */}
        <Show when={filteredVirtualBuckets().length > 0}>
          <div class="mt-4">
            <div class="px-2 py-1 text-[10px] text-gray-500 font-medium uppercase tracking-wider mb-2">Unmapped Endpoints</div>
            <div class="space-y-1">
              <For each={filteredVirtualBuckets()}>
                {(vb) => (
                  <div class="rounded-md overflow-hidden border border-gray-800/50">
                    <div class="flex items-center gap-2 px-3 py-2 bg-orange-900/20 text-sm text-orange-300">
                      <span class="font-mono text-xs">{vb.label}</span>
                      <span class="bg-orange-900/40 text-orange-300 px-2 py-0.5 rounded text-xs ml-auto">{vb.shape_label}</span>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </div>
        </Show>
      </div>

      {!hasAnything() && props.recipeRules().length === 0 && (
        <p class="text-gray-400 mb-4">No shared entities, array properties, or scalar fields detected. You can proceed to name your recipe.</p>
      )}

    </div>
  );
}

function ConstraintRulesEditor(props: {
  rules: Accessor<{ rule: Rule; globalIndex: number }[]>;
  recipeRules: Accessor<Rule[]>;
  setRecipeRules: Setter<Rule[]>;
  entityGraph: Accessor<any>;
  scopedDefs?: string[];
}) {
  // Field options derived from entityGraph().scalar_properties.
  // When scopedDefs is provided, filter to only fields whose def is in the scope.
  const scalarFields = (): { def: string; prop: string; type: string; format: string | null }[] => {
    const g = props.entityGraph();
    if (!g || !Array.isArray(g.scalar_properties)) return [];
    const all = g.scalar_properties.map((sp: any) => ({
      def: sp.def_name,
      prop: sp.prop_name,
      type: sp.prop_type,
      format: sp.format,
    }));
    if (props.scopedDefs) {
      const scopeSet = new Set(props.scopedDefs);
      return all.filter((f: { def: string }) => scopeSet.has(f.def));
    }
    return all;
  };
  const groupedFields = (): GroupedFields => {
    const groups: GroupedFields = {};
    for (const f of scalarFields()) {
      if (!groups[f.def]) groups[f.def] = [];
      groups[f.def].push(f);
    }
    return groups;
  };

  const [newRuleKind, setNewRuleKind] = createSignal<RuleKind>("range");

  const firstFieldPath = (): string => {
    const fs = scalarFields();
    return fs.length > 0 ? `${fs[0].def}.${fs[0].prop}` : "";
  };

  const makeRule = (kind: RuleKind): Rule => {
    const f = firstFieldPath();
    switch (kind) {
      case "range": return { kind: "range", field: f, min: 0, max: 100 };
      case "choice": return { kind: "choice", field: f, options: [] };
      case "const": return { kind: "const", field: f, value: "" };
      case "pattern": return { kind: "pattern", field: f, regex: "" };
      case "compare": return { kind: "compare", left: f, op: "gt", right: f };
    }
  };

  const addRule = () => {
    props.setRecipeRules([...props.recipeRules(), makeRule(newRuleKind())]);
  };

  const removeRule = (globalIdx: number) => {
    props.setRecipeRules(props.recipeRules().filter((_, i) => i !== globalIdx));
  };

  const updateRule = (globalIdx: number, patch: Partial<Rule>) => {
    const next = [...props.recipeRules()];
    next[globalIdx] = { ...next[globalIdx], ...patch } as Rule;
    props.setRecipeRules(next);
  };

  return (
    <div data-testid="constraint-rules-section">
      <div class="flex items-center justify-between mb-1">
        <div class="px-2 py-1 text-[10px] text-gray-500 font-medium uppercase tracking-wider">
          Constraint Rules
          <Show when={props.rules().length > 0}>
            <span class="text-xs text-gray-600 ml-1">{props.rules().length}</span>
          </Show>
        </div>
        <div class="flex items-center gap-2">
          <select
            value={newRuleKind()}
            class="bg-[#070c17] border border-gray-800 rounded px-1.5 py-0.5 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
            onChange={(e) => setNewRuleKind(e.target.value as RuleKind)}
            data-testid="rule-kind-picker"
          >
            <For each={RULE_KINDS}>
              {(k) => <option value={k}>{k}</option>}
            </For>
          </select>
          <button
            class="px-2 py-0.5 text-xs font-medium text-blue-300 hover:text-blue-200 border border-blue-500/30 hover:border-blue-500/60 rounded-md transition-colors disabled:opacity-50"
            onClick={addRule}
            disabled={scalarFields().length === 0}
            data-testid="rule-add-btn"
          >
            + Add Rule
          </button>
        </div>
      </div>

      <div class="space-y-2 ml-2" data-testid="rule-list">
        {/*
          Use <Index> instead of <For>: SolidJS <For> is reference-keyed, so updateRule's
          shallow array-with-spliced-element pattern disposes and recreates the row on every
          keystroke, unmounting the focused <input>. <Index> is index-keyed — it updates an
          internal signal at the same position without re-mounting DOM, preserving focus.
        */}
        <Index each={props.rules()}>
          {(entry) => {
            const rule = () => entry().rule;
            const globalIdx = () => entry().globalIndex;
            // Compare right-side helpers — closed over the reactive `rule` accessor.
            const cr = () => rule() as CompareRule;
            const knownPaths = () => new Set(scalarFields().map((f) => `${f.def}.${f.prop}`));
            const isFieldRef = () => typeof cr().right === "string" && knownPaths().has(cr().right as string);
            return (
              <div class="flex items-center gap-2 px-3 py-2 bg-gray-800/50 rounded-md flex-wrap" data-testid={`rule-row-${rule().kind}`}>
                <span class="text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-blue-500/20 text-blue-300 font-medium w-16 text-center">
                  {rule().kind}
                </span>

                {/* range: field min max */}
                <Show when={rule().kind === "range"}>
                  <FieldSelect
                    value={(rule() as RangeRule).field}
                    onChange={(next) => updateRule(globalIdx(), { field: next } as Partial<RangeRule>)}
                    groupedFields={groupedFields}
                  />
                  <span class="text-[10px] text-gray-500">min</span>
                  <input
                    type="number"
                    value={(rule() as RangeRule).min}
                    class="w-20 bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 text-center focus:outline-none focus:border-gray-700"
                    onInput={(e) => updateRule(globalIdx(), { min: parseFloat(e.currentTarget.value) || 0 } as Partial<RangeRule>)}
                  />
                  <span class="text-[10px] text-gray-500">max</span>
                  <input
                    type="number"
                    value={(rule() as RangeRule).max}
                    class="w-20 bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 text-center focus:outline-none focus:border-gray-700"
                    onInput={(e) => updateRule(globalIdx(), { max: parseFloat(e.currentTarget.value) || 0 } as Partial<RangeRule>)}
                  />
                </Show>

                {/* choice: field options (comma list) */}
                <Show when={rule().kind === "choice"}>
                  <FieldSelect
                    value={(rule() as ChoiceRule).field}
                    onChange={(next) => updateRule(globalIdx(), { field: next } as Partial<ChoiceRule>)}
                    groupedFields={groupedFields}
                  />
                  <input
                    type="text"
                    placeholder="value1, value2, value3"
                    value={stringifyChoiceOptions((rule() as ChoiceRule).options)}
                    class="flex-1 min-w-[160px] bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                    onInput={(e) => updateRule(globalIdx(), { options: parseChoiceOptions(e.currentTarget.value) } as Partial<ChoiceRule>)}
                  />
                </Show>

                {/* const: field value */}
                <Show when={rule().kind === "const"}>
                  <FieldSelect
                    value={(rule() as ConstRule).field}
                    onChange={(next) => updateRule(globalIdx(), { field: next } as Partial<ConstRule>)}
                    groupedFields={groupedFields}
                  />
                  <input
                    type="text"
                    placeholder="value"
                    value={String((rule() as ConstRule).value ?? "")}
                    class="flex-1 min-w-[160px] bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                    onInput={(e) => updateRule(globalIdx(), { value: parseLiteral(e.currentTarget.value) } as Partial<ConstRule>)}
                  />
                </Show>

                {/* pattern: field regex */}
                <Show when={rule().kind === "pattern"}>
                  <FieldSelect
                    value={(rule() as PatternRule).field}
                    onChange={(next) => updateRule(globalIdx(), { field: next } as Partial<PatternRule>)}
                    groupedFields={groupedFields}
                  />
                  <input
                    type="text"
                    placeholder="[A-Z]{3}-[0-9]{4}"
                    value={(rule() as PatternRule).regex}
                    class="flex-1 min-w-[160px] bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 font-mono focus:outline-none focus:border-gray-700"
                    onInput={(e) => updateRule(globalIdx(), { regex: e.currentTarget.value } as Partial<PatternRule>)}
                  />
                </Show>

                {/* compare: left op right (right may be field or literal) */}
                <Show when={rule().kind === "compare"}>
                  <FieldSelect
                    value={(rule() as CompareRule).left}
                    onChange={(next) => updateRule(globalIdx(), { left: next } as Partial<CompareRule>)}
                    groupedFields={groupedFields}
                  />
                  <select
                    value={(rule() as CompareRule).op}
                    class="bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                    onChange={(e) => updateRule(globalIdx(), { op: e.target.value as CompareOp } as Partial<CompareRule>)}
                  >
                    <For each={COMPARE_OPS}>
                      {(o) => <option value={o}>{o}</option>}
                    </For>
                  </select>
                  <select
                    class="bg-[#070c17] border border-gray-800 rounded px-1 py-1 text-[10px] text-gray-100 focus:outline-none focus:border-gray-700"
                    value={isFieldRef() ? "field" : "literal"}
                    onChange={(e) => {
                      if (e.target.value === "field") {
                        updateRule(globalIdx(), { right: firstFieldPath() } as Partial<CompareRule>);
                      } else {
                        updateRule(globalIdx(), { right: "" } as Partial<CompareRule>);
                      }
                    }}
                  >
                    <option value="field">field</option>
                    <option value="literal">literal</option>
                  </select>
                  <Show when={isFieldRef()}>
                    <FieldSelect
                      value={typeof cr().right === "string" ? (cr().right as string) : ""}
                      onChange={(next) => updateRule(globalIdx(), { right: next } as Partial<CompareRule>)}
                      groupedFields={groupedFields}
                    />
                  </Show>
                  <Show when={!isFieldRef()}>
                    <input
                      type="text"
                      placeholder="literal value"
                      value={String(cr().right ?? "")}
                      class="flex-1 min-w-[120px] bg-[#070c17] border border-gray-800 rounded px-2 py-1 text-xs text-gray-100 focus:outline-none focus:border-gray-700"
                      onInput={(e) => updateRule(globalIdx(), { right: parseLiteral(e.currentTarget.value) } as Partial<CompareRule>)}
                    />
                  </Show>
                </Show>

                <button
                  class="ml-auto text-gray-500 hover:text-red-400 text-sm w-6 h-6 flex items-center justify-center rounded hover:bg-red-500/10 transition-colors"
                  onClick={() => removeRule(globalIdx())}
                  title="Remove rule"
                  data-testid="rule-remove-btn"
                >
                  ×
                </button>
              </div>
            );
          }}
        </Index>
      </div>
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

function tryFormatJson(s: string): string {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return s;
  }
}

function humanizeColumn(name: string): string {
  return name
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .replace(/_/g, " ")
    .split(" ")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase())
    .join(" ");
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
  current: "paste" | "select" | "config" | "name";
  onNavigate: (step: "paste" | "select" | "config" | "name") => void;
  editMode?: boolean;
}) {
  const steps: { key: "paste" | "select" | "config" | "name"; label: string; editLabel?: string }[] = [
    { key: "paste", label: "Import" },
    { key: "select", label: "Endpoints" },
    { key: "config", label: "Configure" },
    { key: "name", label: "Generate", editLabel: "Save" },
  ];
  const stepIndex = (key: string) => steps.findIndex((s) => s.key === key);
  const currentIdx = () => stepIndex(props.current);

  return (
    <div class="flex items-center mb-8">
      <For each={steps}>
        {(step, i) => {
          const isActive = () => i() === currentIdx();
          const isCompleted = () => i() < currentIdx();
          const isNavigable = () => props.editMode || isCompleted();
          return (
            <>
              <button
                class={`flex items-center gap-2 px-3 py-1.5 rounded-md text-sm font-medium transition-colors ${
                  isActive()
                    ? "bg-blue-600/20 text-blue-400 ring-1 ring-blue-500/30"
                    : isNavigable()
                    ? "text-gray-300 hover:text-white hover:bg-white/5 cursor-pointer"
                    : "text-gray-600 cursor-default"
                }`}
                onClick={() => { if (isNavigable()) props.onNavigate(step.key); }}
                disabled={!isActive() && !isNavigable()}
              >
                <span class={`flex items-center justify-center w-5 h-5 rounded-full text-xs font-bold ${
                  isActive()
                    ? "bg-blue-600 text-white"
                    : isCompleted()
                    ? "bg-green-600 text-white"
                    : props.editMode
                    ? "bg-gray-700 text-gray-300"
                    : "bg-gray-800 text-gray-600"
                }`}>
                  {isCompleted() ? "\u2713" : i() + 1}
                </span>
                {props.editMode && step.editLabel ? step.editLabel : step.label}
              </button>
              {i() < steps.length - 1 && (
                <div class={`flex-1 h-px mx-1 ${
                  props.editMode
                    ? "bg-blue-600/20"
                    : i() < currentIdx() ? "bg-green-600/40" : "bg-gray-800"
                }`} />
              )}
            </>
          );
        }}
      </For>
    </div>
  );
}

render(() => <App />, document.getElementById("root")!);
