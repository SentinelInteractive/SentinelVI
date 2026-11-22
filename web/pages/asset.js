import { el, fmt, get, post, empty } from "../app.js";

function field(kv, label, value) {
  const text = value === null || value === undefined || value === "" ? "-" : String(value);
  kv.append(el("dt", { text: label }), el("dd", { text }));
}

export async function mount(root, { params }) {
  const id = params[0];
  const title = el("h1", { text: "Asset" });
  const headStatus = el("p", { class: "status", text: `#${id}` });
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [title, headStatus]),
  ]);
  root.append(screen);

  let data;
  try {
    data = await get(`/api/asset/${id}`);
  } catch (e) {
    headStatus.textContent = e.message;
    screen.append(empty("This asset could not be read.", e.message));
    return {};
  }
  const s = data.stream;
  title.textContent = s.name || s.filename || "-";

  const kv = el("dl", { class: "kv" });
  field(kv, "archive", s.archive);
  field(kv, "pack", s.pack);
  field(kv, "offset", s.stream_offset == null ? "-" : fmt.hex(s.stream_offset));
  field(kv, "size", fmt.bytes(s.uncomp_size));
  field(kv, "compressed", fmt.bytes(s.comp_size));
  field(kv, "kind", s.payload_kind);
  field(kv, "magic", s.magic);
  field(kv, "sha1", s.sha1);
  field(kv, "confidence", s.confidence);
  const sourceText = {
    "payload:identifier": "pattern inside the payload",
    "payload:path": "path prefix inside the payload",
    exact: "exact name",
  };
  field(kv, "how matched", sourceText[s.source] || s.source);
  field(
    kv,
    "candidates",
    typeof s.candidates === "number" ? fmt.int(s.candidates) : s.candidates,
  );

  const pre = el("pre", { class: "hex" });
  const exportStatus = el("p", { class: "status" });
  const button = el("button", {
    class: "btn btn-primary",
    type: "button",
    text: "Export payload",
    onclick: async () => {
      button.disabled = true;
      exportStatus.textContent = "Queuing export";
      try {
        const res = await post(`/api/export/${id}`);
        exportStatus.replaceChildren(
          document.createTextNode(`Queued as job ${res.job}. `),
          el("a", { href: "/jobs", text: "Open Jobs" }),
        );
      } catch (e) {
        exportStatus.textContent = e.message;
      }
    },
  });
  screen.append(kv, pre, button, exportStatus);

  try {
    const hex = await get(`/api/asset/${id}/hex?bytes=512`);
    pre.replaceChildren(
      ...(hex.rows || []).map((r) => {
        const off =
          typeof r.offset === "number"
            ? r.offset.toString(16).toUpperCase().padStart(4, "0")
            : String(r.offset);
        return el("div", { text: `${off}  ${r.hex}  ${r.ascii}` });
      }),
    );
  } catch (e) {
    pre.replaceWith(empty("No preview for this payload.", e.message));
    button.disabled = true;
    exportStatus.textContent =
      "This payload is not in the local bin store, so it cannot be exported here.";
  }

  return {};
}
