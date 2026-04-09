import { render } from "solid-js/web";
import { createSignal, onMount, For } from "solid-js";
import "./index.css";

interface Endpoint {
  method: string;
  path: string;
}

interface SpecInfo {
  title: string;
  version: string;
}

function App() {
  const [spec, setSpec] = createSignal<SpecInfo | null>(null);
  const [endpoints, setEndpoints] = createSignal<Endpoint[]>([]);

  onMount(async () => {
    const specRes = await fetch("/_api/admin/spec");
    setSpec(await specRes.json());

    const epRes = await fetch("/_api/admin/endpoints");
    setEndpoints(await epRes.json());
  });

  return (
    <div class="min-h-screen bg-gray-950 text-gray-100 p-8">
      <div class="max-w-3xl mx-auto">
        <h1 class="text-3xl font-bold mb-1">Mirage</h1>
        {spec() && (
          <p class="text-gray-400 mb-8">
            {spec()!.title} v{spec()!.version}
          </p>
        )}
        <h2 class="text-xl font-semibold mb-4">Active Endpoints</h2>
        <table class="w-full text-left">
          <thead>
            <tr class="border-b border-gray-800">
              <th class="py-2 pr-4 text-gray-400 font-medium">Method</th>
              <th class="py-2 text-gray-400 font-medium">Path</th>
            </tr>
          </thead>
          <tbody>
            <For each={endpoints()}>
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
    default: return "bg-gray-800 text-gray-400";
  }
}

render(() => <App />, document.getElementById("root")!);
