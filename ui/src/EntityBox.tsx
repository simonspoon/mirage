import { For, Show } from "solid-js";

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

interface EntityBoxProps {
  name: string;
  properties: Record<string, PropertyInfo>;
  extends?: string;
  x: number;
  y: number;
  width?: number;
  maxVisibleRows?: number;
  onSelectRef?: (refName: string) => void;
}

const ROW_HEIGHT = 24;
const HEADER_HEIGHT = 32;

export default function EntityBox(props: EntityBoxProps) {
  const width = () => props.width ?? 260;
  const maxRows = () => props.maxVisibleRows ?? 10;

  const entries = () => Object.entries(props.properties);
  const hasExtends = () => !!props.extends;
  // Row count: fields + extends pseudo-row (if any) + "No properties" empty-state row (if nothing else)
  const totalRows = () => entries().length + (hasExtends() ? 1 : 0) + (entries().length === 0 && !hasExtends() ? 1 : 0);
  const visibleRows = () => Math.min(totalRows(), maxRows());
  const bodyHeight = () => visibleRows() * ROW_HEIGHT;
  const totalHeight = () => HEADER_HEIGHT + bodyHeight();

  return (
    <g data-entity-box transform={`translate(${props.x}, ${props.y})`}>
      {/* Header background */}
      <rect
        x={0} y={0}
        width={width()} height={HEADER_HEIGHT}
        rx={4} ry={4}
        fill="#1a365d"
      />
      {/* Square off bottom corners of header */}
      <rect
        x={0} y={HEADER_HEIGHT - 4}
        width={width()} height={4}
        fill="#1a365d"
      />
      {/* Header text */}
      <text
        data-entity-box-header
        x={12} y={21}
        fill="#e2e8f0"
        font-size="13"
        font-weight="600"
        font-family="system-ui, -apple-system, sans-serif"
      >
        {props.name.length > 30 ? props.name.slice(0, 28) + "\u2026" : props.name}
      </text>

      {/* Body background */}
      <rect
        x={0} y={HEADER_HEIGHT}
        width={width()} height={bodyHeight()}
        fill="#0d1117"
      />

      {/* Field rows via foreignObject */}
      <foreignObject
        x={0} y={HEADER_HEIGHT}
        width={width()} height={bodyHeight()}
      >
        <div
          data-entity-scroll
          xmlns="http://www.w3.org/1999/xhtml"
          style={{
            "overflow-y": "auto",
            "max-height": `${maxRows() * ROW_HEIGHT}px`,
            "font-family": "system-ui, -apple-system, sans-serif",
            "font-size": "11px",
            "color": "#c9d1d9",
          }}
        >
          {/* Extends pseudo-row */}
          <Show when={hasExtends()}>
            <div
              data-field-row
              data-field-name={`extends:${props.extends}`}
              style={{
                display: "flex",
                "align-items": "center",
                height: `${ROW_HEIGHT}px`,
                padding: "0 8px",
                "border-bottom": "1px solid #21262d",
                gap: "6px",
                "min-width": "0",
              }}
            >
              <span style={{ color: "#8b949e", "flex-shrink": "0", "font-size": "10px" }}>extends</span>
              <span style={{ color: "#8b949e", "flex-shrink": "0" }}>{"\u2192"}</span>
              <span
                data-fk-ref
                style={{
                  padding: "1px 6px",
                  "border-radius": "3px",
                  background: "rgba(139, 92, 246, 0.1)",
                  color: "#a78bfa",
                  "font-family": "ui-monospace, monospace",
                  "font-size": "10px",
                  cursor: props.onSelectRef ? "pointer" : "default",
                  "white-space": "nowrap",
                  overflow: "hidden",
                  "text-overflow": "ellipsis",
                }}
                onClick={() => props.onSelectRef?.(props.extends!)}
              >
                {props.extends}
              </span>
            </div>
          </Show>

          {/* Empty state */}
          <Show when={entries().length === 0 && !hasExtends()}>
            <div
              data-field-row
              data-field-name="__empty__"
              style={{
                display: "flex",
                "align-items": "center",
                height: `${ROW_HEIGHT}px`,
                padding: "0 8px",
                color: "#484f58",
                "font-style": "italic",
              }}
            >
              No properties
            </div>
          </Show>

          {/* Field rows */}
          <For each={entries()}>
            {([fieldName, prop]) => {
              const refTarget = () => prop.ref_name || prop.items_ref;
              const isFK = () => !!refTarget();
              return (
                <div
                  data-field-row
                  data-field-name={fieldName}
                  style={{
                    display: "flex",
                    "align-items": "center",
                    height: `${ROW_HEIGHT}px`,
                    padding: "0 8px",
                    "border-bottom": "1px solid #21262d",
                    gap: "4px",
                    "min-width": "0",
                  }}
                >
                  {/* Field name */}
                  <span
                    style={{
                      "font-weight": prop.required ? "600" : "400",
                      color: prop.required ? "#e2e8f0" : "#8b949e",
                      "white-space": "nowrap",
                      overflow: "hidden",
                      "text-overflow": "ellipsis",
                      "flex-shrink": "1",
                      "min-width": "0",
                      "max-width": "45%",
                    }}
                  >
                    {fieldName}
                  </span>
                  <Show when={prop.required}>
                    <span data-field-required style={{ color: "#f87171", "font-weight": "700", "flex-shrink": "0", "font-size": "11px" }}>*</span>
                  </Show>

                  {/* Spacer */}
                  <span style={{ flex: "1" }} />

                  {/* Array marker */}
                  <Show when={prop.is_array}>
                    <span
                      data-array-marker
                      style={{
                        color: "#fb923c",
                        "font-family": "ui-monospace, monospace",
                        "font-size": "10px",
                        "flex-shrink": "0",
                      }}
                    >[]</span>
                  </Show>

                  {/* FK ref chip */}
                  <Show when={isFK()}>
                    <span
                      data-fk-ref
                      style={{
                        padding: "1px 6px",
                        "border-radius": "3px",
                        background: prop.items_ref ? "rgba(251, 146, 60, 0.1)" : "rgba(139, 92, 246, 0.1)",
                        color: prop.items_ref ? "#fb923c" : "#a78bfa",
                        "font-family": "ui-monospace, monospace",
                        "font-size": "10px",
                        cursor: props.onSelectRef ? "pointer" : "default",
                        "white-space": "nowrap",
                        overflow: "hidden",
                        "text-overflow": "ellipsis",
                        "max-width": "45%",
                        "flex-shrink": "0",
                      }}
                      onClick={() => props.onSelectRef?.(refTarget()!)}
                    >
                      {refTarget()}
                    </span>
                  </Show>

                  {/* Regular type (non-FK) */}
                  <Show when={!isFK()}>
                    <span
                      style={{
                        padding: "1px 6px",
                        "border-radius": "3px",
                        background: prop.enum_values ? "rgba(236, 72, 153, 0.1)" : "rgba(59, 130, 246, 0.08)",
                        color: prop.enum_values ? "#f472b6" : "#7dd3fc",
                        "font-family": "ui-monospace, monospace",
                        "font-size": "10px",
                        "white-space": "nowrap",
                        overflow: "hidden",
                        "text-overflow": "ellipsis",
                        "max-width": "45%",
                        "flex-shrink": "0",
                      }}
                    >
                      {prop.enum_values ? "enum" : prop.type}{prop.format && !prop.enum_values ? ` (${prop.format})` : ""}
                    </span>
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </foreignObject>

      {/* Outer border */}
      <rect
        x={0} y={0}
        width={width()} height={totalHeight()}
        rx={4} ry={4}
        fill="none"
        stroke="#30363d"
        stroke-width={1}
      />
    </g>
  );
}
