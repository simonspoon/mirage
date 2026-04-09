import { render } from "solid-js/web";
import { createSignal, onMount, For, Show } from "solid-js";
import "./index.css";

interface Endpoint {
  method: string;
  path: string;
}

interface SpecInfo {
  title: string;
  version: string;
}

type WizardState = "idle" | "selecting" | "running";

function App() {
  const [state, setState] = createSignal<WizardState>("idle");
  const [specInfo, setSpecInfo] = createSignal<SpecInfo | null>(null);
  const [availableEndpoints, setAvailableEndpoints] = createSignal<Endpoint[]>([]);
  const [selected, setSelected] = createSignal<boolean[]>([]);
  const [activeEndpoints, setActiveEndpoints] = createSignal<Endpoint[]>([]);
  const [seedCount, setSeedCount] = createSignal(10);
  const [error, setError] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);

  onMount(async () => {
    try {
      const specRes = await fetch("/_api/admin/spec");
      const spec: SpecInfo = await specRes.json();
      if (spec.version !== "No spec loaded") {
        setSpecInfo(spec);
        const epRes = await fetch("/_api/admin/endpoints");
        const eps: Endpoint[] = await epRes.json();
        setActiveEndpoints(eps);
        setState("running");
      }
    } catch {
      // Stay in idle state
    }
  });

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
        const err = await res.json();
        setError(err.error || "Import failed");
        setLoading(false);
        return;
      }
      const data = await res.json();
      setSpecInfo(data.spec_info);
      setAvailableEndpoints(data.endpoints);
      setSelected(data.endpoints.map(() => true));
      setSeedCount(10);
      setState("selecting");
    } catch (e) {
      setError("Failed to connect to server.");
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
        const err = await res.json();
        setError(err.error || "Configuration failed");
        setLoading(false);
        return;
      }
      // Fetch the active endpoints from the server (may include auto-registered routes)
      const epRes = await fetch("/_api/admin/endpoints");
      const activeEps: Endpoint[] = await epRes.json();
      setActiveEndpoints(activeEps);
      setState("running");
    } catch {
      setError("Failed to connect to server.");
    }
    setLoading(false);
  };

  const handleReset = () => {
    setError(null);
    setSpecInfo(null);
    setAvailableEndpoints([]);
    setSelected([]);
    setActiveEndpoints([]);
    setState("idle");
  };

  return (
    <div class="min-h-screen bg-gray-950 text-gray-100 p-8">
      <div class="max-w-3xl mx-auto">
        <h1 class="text-3xl font-bold mb-1">Mirage</h1>

        <Show when={error()}>
          <p class="text-red-400 mb-4">{error()}</p>
        </Show>

        {/* State: idle */}
        <Show when={state() === "idle"}>
          <p class="text-gray-400 mb-6">Paste a Swagger 2.0 spec to get started</p>
          <textarea
            id="spec-input"
            class="w-full h-64 bg-gray-900 border border-gray-700 rounded p-3 font-mono text-sm text-gray-100 resize-y"
            placeholder="Paste Swagger 2.0 YAML or JSON here..."
          />
          <button
            id="import-btn"
            class="mt-4 px-6 py-2 bg-blue-600 hover:bg-blue-500 rounded font-medium disabled:opacity-50"
            onClick={handleImport}
            disabled={loading()}
          >
            {loading() ? "Importing..." : "Import Spec"}
          </button>
        </Show>

        {/* State: selecting */}
        <Show when={state() === "selecting"}>
          <p class="text-gray-400 mb-6">
            {specInfo()?.title} v{specInfo()?.version}
          </p>
          <h2 class="text-xl font-semibold mb-4">Select Endpoints</h2>
          <div id="endpoint-list" class="space-y-1">
            <For each={availableEndpoints()}>
              {(ep, i) => (
                <label class="flex items-center gap-3 py-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={selected()[i()]}
                    onChange={() => toggleEndpoint(i())}
                    class="endpoint-checkbox accent-blue-500"
                  />
                  <span class={`font-mono text-sm px-2 py-0.5 rounded ${methodColor(ep.method)}`}>
                    {ep.method.toUpperCase()}
                  </span>
                  <span class="font-mono text-sm">{ep.path}</span>
                </label>
              )}
            </For>
          </div>
          <div class="mt-4 flex items-center gap-4">
            <label class="flex items-center gap-2 text-sm">
              Seed count:
              <input
                id="seed-count"
                type="number"
                value={seedCount()}
                min={1}
                max={100}
                onInput={(e) => setSeedCount(parseInt(e.currentTarget.value) || 1)}
                class="w-20 bg-gray-900 border border-gray-700 rounded px-2 py-1 text-gray-100"
              />
            </label>
            <button
              id="start-btn"
              class="px-6 py-2 bg-green-600 hover:bg-green-500 rounded font-medium disabled:opacity-50"
              onClick={handleConfigure}
              disabled={loading()}
            >
              {loading() ? "Configuring..." : "Start Mock Server"}
            </button>
          </div>
        </Show>

        {/* State: running */}
        <Show when={state() === "running"}>
          <p class="text-gray-400 mb-8">
            {specInfo()?.title} v{specInfo()?.version}
          </p>
          <h2 class="text-xl font-semibold mb-4">Active Endpoints</h2>
          <table class="w-full text-left">
            <thead>
              <tr class="border-b border-gray-800">
                <th class="py-2 pr-4 text-gray-400 font-medium">Method</th>
                <th class="py-2 text-gray-400 font-medium">Path</th>
              </tr>
            </thead>
            <tbody>
              <For each={activeEndpoints()}>
                {(ep) => (
                  <tr class="border-b border-gray-800/50">
                    <td class="py-2 pr-4">
                      <span class={`font-mono text-sm px-2 py-0.5 rounded ${methodColor(ep.method)}`}>
                        {ep.method.toUpperCase()}
                      </span>
                    </td>
                    <td class="py-2 font-mono text-sm">{ep.path}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
          <button
            id="reset-btn"
            class="mt-6 px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded text-sm"
            onClick={handleReset}
          >
            Import New Spec
          </button>
        </Show>
      </div>
    </div>
  );
}

function methodColor(method: string): string {
  switch (method.toLowerCase()) {
    case "get": return "bg-green-900/50 text-green-400";
    case "post": return "bg-blue-900/50 text-blue-400";
    case "delete": return "bg-red-900/50 text-red-400";
    case "put": return "bg-yellow-900/50 text-yellow-400";
    case "patch": return "bg-purple-900/50 text-purple-400";
    default: return "bg-gray-800 text-gray-400";
  }
}

render(() => <App />, document.getElementById("root")!);
