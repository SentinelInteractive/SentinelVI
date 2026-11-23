import { el, fmt, get, empty } from "../app.js";

const PROVENANCE =
  "This path was matched using a stored hash. It is not an original archive path.";

function slotFields(s) {
  return [
    ["root", s.root],
    ["archive offset", fmt.hex(s.archive_offset)],
    ["slot", fmt.int(s.slot)],
    ["absolute data offset", fmt.hex(s.data_offset_absolute)],
    ["relative data offset", fmt.hex(s.data_offset_relative)],
    ["stored bytes", fmt.int(s.stored_bytes)],
    ["logical bytes", fmt.int(s.logical_bytes)],
    ["kind", s.kind],
    ["hash32", s.hash32],
    ["hash64", s.hash64],
    ["name origin", s.name_origin],
    ["signature status", s.signature_status],
  ];
}

function fallbackFields(s) {
  return [
    ["archive", s.archive],
    ["pack", s.pack],
    ["offset", fmt.hex(s.stream_offset)],
    ["logical bytes", fmt.int(s.uncomp_size)],
    ["stored bytes", fmt.int(s.comp_size)],
    ["kind", s.payload_kind],
    ["magic", s.magic],
    ["sha1", s.sha1],
    ["confidence", s.confidence],
    ["how matched", s.source],
    ["candidates", fmt.int(s.candidates)],
  ];
}

export async function mount(root, { params, mode }) {
  const isToc = mode === "toc";
  const id = params[0];
  const url = isToc
    ? `/api/library/slots/${encodeURIComponent(id)}`
    : `/api/asset/${encodeURIComponent(id)}`;

  const title = el("h1", { text: isToc ? "Slot" : "Asset" });
  const status = el("p", { class: "status", text: "Loading metadata" });
  const screen = el("div", { class: "screen" }, [
    el("div", { class: "screen-head" }, [title, status]),
  ]);
  root.append(screen);

  let body;
  try {
    body = await get(url);
  } catch (e) {
    status.textContent = e.message || "The record could not be read.";
    status.dataset.state = "failed";
    screen.append(empty("This record could not be read.", e.message));
    return {};
  }

  const record = isToc ? body.slot : body.stream;
  if (!record) {
    status.textContent = "";
    screen.append(empty("No metadata is stored for this record.", "Open another record from the Library."));
    return {};
  }
  title.textContent = isToc ? record.display_name : record.name || record.filename || `Payload ${id}`;
  status.textContent = isToc ? "TOC metadata" : "Reconstructed catalog";

  const kv = el("dl", { class: "kv" });
  const fields = isToc ? slotFields(record) : fallbackFields(record);
  for (const [label, value] of fields) {
    kv.append(el("dt", { text: label }), el("dd", { text: value ?? "-" }));
  }
  if (record.name_origin === "DictionaryMatch") {
    screen.append(el("p", { class: "hint", text: PROVENANCE }));
  }
  screen.append(kv);

  return {};
}
