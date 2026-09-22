use crate::layout::LayoutNode;
use crate::parser::Element;
use std::collections::HashMap;
use std::io::Write;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,

    compressed_dom: Option<Vec<u8>>,

    has_dom: bool,
}

impl HistoryEntry {
    pub fn new(url: String, title: String, dom: &Element) -> Self {
        let compressed_dom = Self::compress_dom(dom);
        let has_dom = compressed_dom.is_some();
        Self {
            url,
            title,
            compressed_dom,
            has_dom,
        }
    }

    pub fn get_dom(&self) -> Option<Element> {
        self.compressed_dom
            .as_ref()
            .and_then(|data| decompress_dom_bytes(data).ok())
    }

    pub fn has_dom(&self) -> bool {
        self.has_dom
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.url.capacity())
            .saturating_add(self.title.capacity())
            .saturating_add(self.compressed_dom.as_ref().map_or(0, Vec::capacity))
    }

    fn compress_dom(dom: &Element) -> Option<Vec<u8>> {
        if dom.children.is_empty() && dom.tag == "root" {
            return None;
        }

        const MAX_SNAPSHOT_JSON_BYTES: usize = 8 * 1024 * 1024;
        let json = serde_json::to_vec(dom).ok()?;
        if json.len() > MAX_SNAPSHOT_JSON_BYTES {
            log::warn!(
                "Skipping history DOM snapshot: {} bytes exceeds the cap",
                json.len()
            );
            return None;
        }

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&json).ok()?;
        let compressed = encoder.finish().ok()?;

        if compressed.len() < json.len() {
            Some(compressed)
        } else {
            Some(json)
        }
    }
}

fn decompress_dom_bytes(data: &[u8]) -> Result<Element, Box<dyn std::error::Error>> {
    const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];

    let json = if data.starts_with(GZIP_MAGIC) {
        let mut decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(data));
        std::io::read_to_string(&mut decoder)
            .map_err(|e| format!("Failed to decompress DOM data: {}", e))?
    } else {
        String::from_utf8(data.to_vec()).map_err(|e| format!("Failed to read DOM JSON: {}", e))?
    };

    if json_nesting_depth(&json) > SNAPSHOT_MAX_JSON_DEPTH {
        return Err(format!(
            "DOM snapshot too deeply nested (>{}) to restore safely",
            SNAPSHOT_MAX_JSON_DEPTH
        )
        .into());
    }
    let mut de = serde_json::Deserializer::from_str(&json);
    de.disable_recursion_limit();
    let dom: Element = serde::Deserialize::deserialize(&mut de)
        .map_err(|e| format!("Failed to parse decompressed DOM JSON: {}", e))?;

    Ok(flatten_deep(dom, crate::parser::MAX_DOM_DEPTH))
}

const SNAPSHOT_MAX_JSON_DEPTH: usize = 1024;

fn json_nesting_depth(json: &str) -> usize {
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for b in json.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max_depth
}

fn flatten_deep(root: Element, max_depth: usize) -> Element {
    let mut flat: Vec<(Element, usize)> = Vec::new();
    let mut st: Vec<(Element, usize)> = vec![(root, 0)];
    while let Some((mut el, depth)) = st.pop() {
        let children = std::mem::take(&mut el.children);
        for child in children.into_iter().rev() {
            st.push((child, depth + 1));
        }
        flat.push((el, depth));
    }

    let mut stack: Vec<Element> = Vec::new();
    let mut depths: Vec<usize> = Vec::new();
    for (el, raw_depth) in flat {
        let eff_depth = raw_depth.min(max_depth);

        while let Some(&top) = depths.last() {
            if top < eff_depth {
                break;
            }
            let Some(child) = stack.pop() else {
                break;
            };
            depths.pop();
            if let Some(parent) = stack.last_mut() {
                parent.add_child(child);
            }
        }
        stack.push(el);
        depths.push(eff_depth);
    }
    while stack.len() > 1 {
        let Some(child) = stack.pop() else {
            break;
        };
        depths.pop();
        if let Some(parent) = stack.last_mut() {
            parent.add_child(child);
        } else {
            stack.push(child);
            break;
        }
    }
    stack.pop().unwrap_or_else(|| Element::new("html"))
}

#[derive(Debug)]
pub struct Tab {
    pub id: usize,
    pub url: String,
    pub title: String,

    pub dom: Element,

    pub layout: Option<LayoutNode>,

    pub incognito: bool,

    pub is_error: bool,

    history: Vec<HistoryEntry>,

    history_pos: usize,

    pub is_pinned: bool,

    pub is_sleeping: bool,

    pub last_active_timestamp: i64,

    pub slept_at: Option<i64>,

    pub is_audible: bool,

    pub is_muted: bool,

    pub group_id: Option<u64>,

    pub is_discarded: bool,

    pub compressed_dom: Option<Vec<u8>>,

    pub runtime: Option<crate::web_runtime::PageRuntime>,
}

#[derive(Debug, PartialEq)]
pub enum WakeResult {
    NotSleeping,

    RestoredFromCache,

    NeedsReload(String),
}

const MAX_HISTORY_ENTRIES: usize = 60;

impl Tab {
    pub fn new(id: usize, url: String, dom: Element, title: String) -> Self {
        let entry = HistoryEntry::new(url.clone(), title.clone(), &dom);
        Tab {
            id,
            url: url.clone(),
            title,
            dom,
            layout: None,
            incognito: false,
            is_error: false,
            history: vec![entry],
            history_pos: 0,
            is_pinned: false,
            is_sleeping: false,
            last_active_timestamp: chrono::Utc::now().timestamp(),
            slept_at: None,
            is_audible: false,
            is_muted: false,
            group_id: None,
            is_discarded: false,
            compressed_dom: None,
            runtime: None,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn set_url(&mut self, url: String) {
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }
        self.url = url.clone();
        self.layout = None;
        self.runtime = None;
    }

    pub fn attach_runtime(&mut self, runtime: crate::web_runtime::PageRuntime) {
        self.runtime = Some(runtime);
    }

    pub fn init_runtime(
        &mut self,
        css_rules: Vec<crate::css_parser::CssRule>,
        viewport_width: u32,
        base_url: &str,
    ) -> Result<(), String> {
        let mut runtime = crate::web_runtime::PageRuntime::from_element(
            &self.dom,
            css_rules,
            viewport_width,
            base_url,
        )?;
        let _ = runtime.run_document();
        self.dom = runtime.dom_element();
        self.runtime = Some(runtime);
        Ok(())
    }

    pub fn pump_runtime(&mut self, elapsed_ms: u64) -> Result<usize, String> {
        if let Some(ref mut runtime) = self.runtime {
            let processed = runtime.pump_events(elapsed_ms)?;
            self.dom = runtime.dom_element();
            Ok(processed)
        } else {
            Ok(0)
        }
    }

    pub fn evaluate_js(&mut self, script: &str) -> Result<crate::javascript::JsvValue, String> {
        if let Some(ref mut runtime) = self.runtime {
            let result = runtime.evaluate(script)?;
            self.dom = runtime.dom_element();
            Ok(result)
        } else {
            Err("No active runtime on tab".to_string())
        }
    }

    pub fn dispatch_event(
        &mut self,
        target: u64,
        event_type: &str,
    ) -> Result<crate::live_dom::DispatchReport, String> {
        if let Some(ref mut runtime) = self.runtime {
            let report = runtime.dispatch_event_by_type(target, event_type, None)?;
            self.dom = runtime.dom_element();
            Ok(report)
        } else {
            Err("No active runtime on tab".to_string())
        }
    }

    pub fn runtime_heap_bytes(&self) -> usize {
        self.runtime.as_ref().map(|r| r.heap_bytes()).unwrap_or(0)
    }

    pub fn push_history(&mut self, entry: HistoryEntry) {
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }

        if let Some(last) = self.history.last_mut() {
            if last.url == entry.url {
                *last = entry;
                self.history_pos = self.history.len() - 1;
                self.layout = None;
                return;
            }
        }
        self.history.push(entry);

        if self.history.len() > MAX_HISTORY_ENTRIES {
            self.history.remove(0);
        }
        self.history_pos = self.history.len() - 1;
        self.layout = None;
    }

    pub fn go_back(&mut self) -> bool {
        if self.is_error {
            if let Some(entry) = self.history.get(self.history_pos) {
                self.url = entry.url.clone();
                self.title = entry.title.clone();
                self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
                self.layout = None;
                self.is_error = false;
                return true;
            }
            return false;
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let entry = &self.history[self.history_pos];
            self.url = entry.url.clone();
            self.title = entry.title.clone();
            self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
            self.layout = None;
            self.is_error = false;
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if self.history_pos + 1 < self.history.len() {
            self.history_pos += 1;
            let entry = &self.history[self.history_pos];
            self.url = entry.url.clone();
            self.title = entry.title.clone();
            self.dom = entry.get_dom().unwrap_or_else(|| Element::new("root"));
            self.layout = None;
            self.is_error = false;
            true
        } else {
            false
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    pub fn history_retained_bytes(&self) -> usize {
        self.history.iter().fold(0usize, |total, entry| {
            total.saturating_add(entry.retained_bytes())
        })
    }

    pub(crate) fn compressed_snapshot_bytes(&self) -> usize {
        self.compressed_dom.as_ref().map_or(0, |d| d.len())
    }

    pub fn sleep(&mut self) -> usize {
        if self.is_sleeping {
            return 0;
        }

        let dom_nodes = crate::count_elements(&self.dom);
        let layout_nodes = self
            .layout
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);

        let original_size = dom_nodes * 210 + layout_nodes * 320;

        self.compressed_dom = self.compress_dom();

        let compressed_size = self.compressed_dom.as_ref().map_or(0, |d| d.len());
        let freed_bytes = original_size.saturating_sub(compressed_size);

        self.dom = Element::new("root");
        self.layout = None;
        self.runtime = None;
        self.is_sleeping = true;
        self.slept_at = Some(chrono::Utc::now().timestamp());

        freed_bytes
    }

    pub fn wake(&mut self) -> WakeResult {
        if !self.is_sleeping {
            return WakeResult::NotSleeping;
        }

        self.is_sleeping = false;
        self.slept_at = None;
        self.last_active_timestamp = chrono::Utc::now().timestamp();
        self.layout = None;

        if let Some(ref compressed) = self.compressed_dom {
            if let Ok(dom) = Self::decompress_dom(compressed) {
                self.dom = dom;
                self.compressed_dom = None;
                return WakeResult::RestoredFromCache;
            }
        }

        self.dom = Element::new("root");
        self.compressed_dom = None;

        WakeResult::NeedsReload(self.url.clone())
    }

    pub fn can_sleep(&self) -> bool {
        if self.is_sleeping {
            return false;
        }
        if self.is_discarded {
            return false;
        }
        if self.is_audible {
            return false;
        }
        if self.is_pinned {
            return false;
        }
        if self.is_error {
            return false;
        }
        if self.url.starts_with("sylphra://") || self.url.starts_with("ghita://") {
            return false;
        }
        if self.url.is_empty() || self.url == "about:blank" {
            return false;
        }
        true
    }

    pub fn is_memory_relief_protected(&self, active_tab_id: Option<usize>) -> bool {
        Some(self.id) == active_tab_id
            || self.is_pinned
            || self.is_audible
            || self.is_error
            || self.url.starts_with("sylphra://")
            || self.url.starts_with("ghita://")
            || self.is_discarded
    }

    pub fn seconds_since_active(&self) -> i64 {
        let now = chrono::Utc::now().timestamp();
        now - self.last_active_timestamp
    }

    pub fn mark_active(&mut self) {
        self.last_active_timestamp = chrono::Utc::now().timestamp();
    }

    fn compress_dom(&self) -> Option<Vec<u8>> {
        if self.dom.children.is_empty() && self.dom.tag == "root" {
            return None;
        }

        const MAX_SNAPSHOT_JSON_BYTES: usize = 8 * 1024 * 1024;
        let json = match serde_json::to_vec(&self.dom) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("Failed to compress DOM for {}: {}", self.url, e);
                return None;
            }
        };
        if json.len() > MAX_SNAPSHOT_JSON_BYTES {
            log::warn!(
                "Skipping sleep snapshot for {}: {} bytes exceeds the cap",
                self.url,
                json.len()
            );
            return None;
        }

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        match encoder.write_all(&json) {
            Ok(_) => match encoder.finish() {
                Ok(compressed) => {
                    if compressed.len() < json.len() {
                        Some(compressed)
                    } else {
                        log::warn!(
                            "Failed to compress DOM for {}: compression not beneficial",
                            self.url
                        );
                        Some(json)
                    }
                }
                Err(e) => {
                    log::warn!("Failed to finish gzip encoding for {}: {}", self.url, e);
                    Some(json)
                }
            },
            Err(e) => {
                log::warn!("Failed to write to gzip encoder for {}: {}", self.url, e);
                Some(json)
            }
        }
    }

    fn decompress_dom(data: &[u8]) -> Result<Element, Box<dyn std::error::Error>> {
        decompress_dom_bytes(data)
    }

    pub fn discard_score(&self, active_tab_id: Option<usize>) -> i64 {
        if self.is_discarded {
            return -1000;
        }
        if Some(self.id) == active_tab_id {
            return -10000;
        }

        let mut score: i64 = 0;

        let minutes_inactive = self.seconds_since_active() / 60;
        score += minutes_inactive * 10;

        if self.is_pinned {
            score -= 100;
        }
        if self.is_audible {
            score -= 200;
        }

        if self.is_sleeping {
            score += 50;
        }

        if self.url.starts_with("sylphra://") || self.url.starts_with("ghita://") {
            score += 20;
        }

        score
    }

    pub fn discard(&mut self) -> usize {
        if self.is_discarded {
            return 0;
        }

        let dom_nodes = crate::count_elements(&self.dom);
        let layout_nodes = self
            .layout
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);
        let snapshot_bytes = self.compressed_dom.as_ref().map_or(0, |d| d.len());
        let freed_bytes = dom_nodes * 210 + layout_nodes * 320 + snapshot_bytes;

        self.dom = Element::new("root");
        self.layout = None;
        self.runtime = None;

        self.compressed_dom = None;
        self.is_sleeping = false;
        self.is_discarded = true;
        self.slept_at = None;

        freed_bytes
    }

    pub fn undiscard(&mut self) -> Option<String> {
        if !self.is_discarded {
            return None;
        }

        self.is_discarded = false;
        self.last_active_timestamp = chrono::Utc::now().timestamp();
        self.dom = Element::new("root");
        self.layout = None;

        Some(self.url.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TabGroup {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
}

pub struct TabManager {
    tabs: HashMap<usize, Tab>,
    active_tab_id: Option<usize>,
    next_id: usize,

    tab_order: Vec<usize>,

    closed_tabs: Vec<(String, String)>,
    groups: HashMap<u64, TabGroup>,
    next_group_id: u64,
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TabManager {
    pub fn new() -> Self {
        TabManager {
            tabs: HashMap::new(),
            active_tab_id: None,
            next_id: 1,
            tab_order: Vec::new(),
            closed_tabs: Vec::new(),
            groups: HashMap::new(),
            next_group_id: 1,
        }
    }

    pub fn add_tab(&mut self, url: &str, dom: Element, title: &str) -> usize {
        let id = self.next_id;
        self.next_id += 1;

        let tab = Tab::new(id, url.to_string(), dom, title.to_string());
        self.tabs.insert(id, tab);
        self.tab_order.push(id);

        self.set_active_tab(id);
        id
    }

    pub fn get_tab(&self, id: usize) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    pub fn get_tab_mut(&mut self, id: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(&id)
    }

    pub fn get_tab_by_index(&self, index: usize) -> Option<&Tab> {
        self.tab_order.get(index).and_then(|id| self.tabs.get(id))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_tab_id.and_then(|id| self.tabs.get(&id))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.active_tab_id.and_then(|id| self.tabs.get_mut(&id))
    }

    pub fn active_tab_id(&self) -> Option<usize> {
        self.active_tab_id
    }

    pub fn set_active_tab(&mut self, id: usize) {
        if self.tabs.contains_key(&id) {
            self.active_tab_id = Some(id);
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.mark_active();
            }
        }
    }

    pub fn set_active_by_index(&mut self, index: usize) {
        if let Some(&id) = self.tab_order.get(index) {
            self.active_tab_id = Some(id);
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.mark_active();
            }
        }
    }

    pub fn activate_next(&mut self) {
        if self.tab_order.is_empty() {
            return;
        }
        let pos = self
            .active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0);
        let next = (pos + 1) % self.tab_order.len();
        self.active_tab_id = Some(self.tab_order[next]);

        if let Some(tab) = self.get_tab_mut(self.tab_order[next]) {
            tab.mark_active();
        }
    }

    pub fn activate_prev(&mut self) {
        if self.tab_order.is_empty() {
            return;
        }
        let pos = self
            .active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0);
        let prev = (pos + self.tab_order.len() - 1) % self.tab_order.len();
        self.active_tab_id = Some(self.tab_order[prev]);
        if let Some(tab) = self.get_tab_mut(self.tab_order[prev]) {
            tab.mark_active();
        }
    }

    pub fn remove_tab(&mut self, id: usize) -> Option<Tab> {
        let old_pos = self.tab_order.iter().position(|&tid| tid == id);

        self.tab_order.retain(|&tid| tid != id);

        if self.active_tab_id == Some(id) {
            if self.tab_order.is_empty() {
                self.active_tab_id = None;
            } else {
                let idx = old_pos.unwrap_or(0).min(self.tab_order.len() - 1);
                self.active_tab_id = Some(self.tab_order[idx]);
            }
        }

        let removed = self.tabs.remove(&id);

        if let Some(ref tab) = removed {
            if !tab.incognito && (tab.url.starts_with("http://") || tab.url.starts_with("https://"))
            {
                self.closed_tabs.push((tab.url.clone(), tab.title.clone()));
                if self.closed_tabs.len() > 25 {
                    self.closed_tabs.remove(0);
                }
            }
        }

        removed
    }

    pub fn pop_closed_tab(&mut self) -> Option<(String, String)> {
        self.closed_tabs.pop()
    }

    pub fn has_closed_tabs(&self) -> bool {
        !self.closed_tabs.is_empty()
    }

    pub fn close_all_tabs(&mut self) {
        self.tabs.clear();
        self.tab_order.clear();
        self.active_tab_id = None;
        self.next_id = 1;
        self.groups.clear();
        self.next_group_id = 1;
    }

    pub fn active_title(&self) -> Option<String> {
        self.active_tab().map(|t| t.title.clone())
    }

    pub fn active_url(&self) -> Option<String> {
        self.active_tab().map(|t| t.url.clone())
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn all_tabs(&self) -> std::collections::hash_map::Values<'_, usize, Tab> {
        self.tabs.values()
    }

    pub fn active_index(&self) -> usize {
        self.active_tab_id
            .and_then(|id| self.tab_order.iter().position(|&tid| tid == id))
            .unwrap_or(0)
    }

    pub fn iter_tabs(&self) -> Vec<&Tab> {
        self.tab_order
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tab_order.iter().filter_map(|id| self.tabs.get(id))
    }

    pub fn groups(&self) -> &HashMap<u64, TabGroup> {
        &self.groups
    }

    pub fn pin_tab_by_index(&mut self, index: usize, pinned: bool) -> bool {
        let Some(tab_id) = self.tab_order.get(index).copied() else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        if tab.is_pinned == pinned {
            return true;
        }
        tab.is_pinned = pinned;
        self.tab_order.remove(index);
        let pinned_count = self
            .tab_order
            .iter()
            .filter(|id| self.tabs.get(id).is_some_and(|tab| tab.is_pinned))
            .count();

        self.tab_order.insert(pinned_count, tab_id);
        true
    }

    pub fn toggle_mute_by_index(&mut self, index: usize) -> Option<bool> {
        let tab_id = *self.tab_order.get(index)?;
        let tab = self.tabs.get_mut(&tab_id)?;
        tab.is_muted = !tab.is_muted;
        Some(tab.is_muted)
    }

    pub fn create_group(&mut self, name: &str, color: &str) -> Result<u64, String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("Tab group name must contain 1-64 bytes".to_string());
        }
        if color.len() > 32 {
            return Err("Tab group color exceeds 32 bytes".to_string());
        }
        let id = self.next_group_id;
        self.next_group_id = self.next_group_id.saturating_add(1);
        self.groups.insert(
            id,
            TabGroup {
                id,
                name: name.to_string(),
                color: color.to_string(),
                collapsed: false,
            },
        );
        Ok(id)
    }

    pub fn assign_tab_to_group_by_index(&mut self, index: usize, group_id: Option<u64>) -> bool {
        if group_id.is_some_and(|id| !self.groups.contains_key(&id)) {
            return false;
        }
        let Some(tab_id) = self.tab_order.get(index).copied() else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        tab.group_id = group_id;
        true
    }

    pub fn reorder_tab(&mut self, from_index: usize, to_index: usize) -> bool {
        if from_index >= self.tab_order.len() || to_index >= self.tab_order.len() {
            return false;
        }
        let moving_id = self.tab_order[from_index];
        let moving_pinned = self.tabs.get(&moving_id).is_some_and(|tab| tab.is_pinned);
        let target_id = self.tab_order[to_index];
        let target_pinned = self.tabs.get(&target_id).is_some_and(|tab| tab.is_pinned);
        if moving_pinned != target_pinned {
            return false;
        }
        let tab_id = self.tab_order.remove(from_index);
        self.tab_order.insert(to_index, tab_id);
        true
    }

    pub fn session_snapshot(&self) -> crate::storage::BrowserSession {
        crate::storage::BrowserSession {
            active_index: self.active_index(),
            tabs: self
                .iter()
                .filter(|tab| {
                    !tab.incognito
                        && matches!(
                            url::Url::parse(&tab.url).ok().map(|url| url.scheme().to_string()),
                            Some(scheme) if matches!(scheme.as_str(), "http" | "https" | "file")
                        )
                })
                .map(|tab| crate::storage::SessionTab {
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    pinned: tab.is_pinned,
                    muted: tab.is_muted,
                    group_id: tab.group_id,
                })
                .collect(),
            groups: self
                .groups
                .values()
                .map(|group| crate::storage::SessionTabGroup {
                    id: group.id,
                    name: group.name.clone(),
                    color: group.color.clone(),
                    collapsed: group.collapsed,
                })
                .collect(),
        }
    }

    pub fn restore_session(&mut self, session: &crate::storage::BrowserSession) -> usize {
        self.close_all_tabs();
        for group in &session.groups {
            self.next_group_id = self.next_group_id.max(group.id.saturating_add(1));
            self.groups.insert(
                group.id,
                TabGroup {
                    id: group.id,
                    name: group.name.clone(),
                    color: group.color.clone(),
                    collapsed: group.collapsed,
                },
            );
        }
        for saved in session.tabs.iter().take(100) {
            let id = self.add_tab(
                &saved.url,
                Element::new("root"),
                if saved.title.trim().is_empty() {
                    &saved.url
                } else {
                    &saved.title
                },
            );
            if let Some(tab) = self.tabs.get_mut(&id) {
                tab.is_pinned = saved.pinned;
                tab.is_muted = saved.muted;
                tab.group_id = saved.group_id.filter(|id| self.groups.contains_key(id));
            }
        }
        self.tab_order
            .sort_by_key(|id| !self.tabs.get(id).is_some_and(|tab| tab.is_pinned));
        if !self.tab_order.is_empty() {
            let index = session.active_index.min(self.tab_order.len() - 1);
            self.set_active_by_index(index);
        }
        self.tab_order.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Element;

    #[test]
    fn test_tab_manager_creation() {
        let mut tm = TabManager::new();
        let dom = Element::new("body");
        let id = tm.add_tab("https://example.com", dom, "Example");
        assert_eq!(tm.tab_count(), 1);
        assert_eq!(id, 1);
    }

    #[test]
    fn test_tab_navigation() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );

        assert!(!tab.can_go_back());

        let entry_b = HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry_b);

        let entry_c = HistoryEntry::new(
            "https://c.com".to_string(),
            "C".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry_c);

        assert!(tab.can_go_back());
        assert!(!tab.can_go_forward());

        tab.go_back();
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
    }

    #[test]
    fn test_tab_set_url_bounds() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );
        tab.set_url("https://b.com".to_string());
        assert_eq!(tab.url, "https://b.com");
    }

    #[test]
    fn test_push_history_dedups_same_url() {
        let mut tab = Tab::new(
            1,
            "https://a.com".to_string(),
            Element::new("body"),
            "A".to_string(),
        );

        let entry = HistoryEntry::new(
            "https://a.com".to_string(),
            "A (reloaded)".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry);
        assert_eq!(tab.history.len(), 1);
        assert_eq!(tab.history[0].title, "A (reloaded)");

        let entry_b = HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry_b);
        assert_eq!(tab.history.len(), 2);

        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(!tab.can_go_back());
    }

    #[test]
    fn test_back_from_error_returns_to_last_good_page() {
        let mut tab = Tab::new(
            1,
            "https://newtab".to_string(),
            Element::new("body"),
            "New Tab".to_string(),
        );
        let entry_a = HistoryEntry::new(
            "https://a.com".to_string(),
            "A".to_string(),
            &Element::new("body"),
        );
        tab.push_history(entry_a);

        tab.url = "https://b.com".to_string();
        tab.is_error = true;

        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(!tab.is_error);
        assert!(tab.can_go_back());
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://newtab");
        assert!(!tab.can_go_back());
    }

    #[test]
    fn test_navigation_clears_forward_history() {
        let mut tab = Tab::new(
            1,
            "https://newtab".to_string(),
            Element::new("body"),
            "New Tab".to_string(),
        );
        for url in ["https://a.com", "https://b.com", "https://c.com"] {
            tab.push_history(HistoryEntry::new(
                url.to_string(),
                url.to_string(),
                &Element::new("body"),
            ));
        }

        assert!(tab.go_back());
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
        tab.push_history(HistoryEntry::new(
            "https://d.com".to_string(),
            "D".to_string(),
            &Element::new("body"),
        ));
        assert_eq!(tab.url, "https://b.com");
        assert!(!tab.can_go_forward());
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://b.com");
        assert!(tab.can_go_forward());
        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
    }

    #[test]
    fn test_tab_manager_order() {
        let mut tm = TabManager::new();
        let _id1 = tm.add_tab("https://a.com", Element::new("body"), "A");
        let id2 = tm.add_tab("https://b.com", Element::new("div"), "B");
        let id3 = tm.add_tab("https://c.com", Element::new("span"), "C");

        assert_eq!(tm.tab_count(), 3);
        assert_eq!(tm.active_tab_id(), Some(id3));

        assert_eq!(tm.get_tab_by_index(0).unwrap().url, "https://a.com");
        assert_eq!(tm.get_tab_by_index(1).unwrap().url, "https://b.com");
        assert_eq!(tm.get_tab_by_index(2).unwrap().url, "https://c.com");

        tm.remove_tab(id2);
        assert_eq!(tm.tab_count(), 2);
        assert_eq!(tm.get_tab_by_index(0).unwrap().url, "https://a.com");
        assert_eq!(tm.get_tab_by_index(1).unwrap().url, "https://c.com");
    }

    #[test]
    fn test_tab_sleep_drops_dom_and_layout() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Example".to_string(),
        );

        assert!(!tab.is_sleeping);
        assert!(tab.dom.find_tag("h1").is_some());
        assert_eq!(tab.url, "https://example.com");

        let freed = tab.sleep();
        assert!(tab.is_sleeping);
        assert!(freed > 0, "Sleep should free some bytes");

        assert!(tab.dom.find_tag("h1").is_none());
        assert_eq!(tab.dom.tag, "root");

        assert!(tab.layout.is_none());

        assert_eq!(tab.url, "https://example.com");
        assert_eq!(tab.title, "Example");

        assert!(tab.slept_at.is_some());
    }

    #[test]
    fn test_tab_sleep_is_idempotent() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let freed1 = tab.sleep();
        assert!(freed1 > 0);
        assert!(tab.is_sleeping);

        let freed2 = tab.sleep();
        assert_eq!(freed2, 0);
    }

    #[test]
    fn test_tab_wake_restores_from_compressed() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.sleep();
        assert!(tab.is_sleeping);

        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!tab.is_sleeping);
        assert!(tab.slept_at.is_none());

        assert_ne!(tab.dom.tag, "root");
        assert!(tab.dom.find_tag("html").is_some());
        assert!(tab.layout.is_none());
    }

    #[test]
    fn test_tab_wake_noop_when_not_sleeping() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::NotSleeping),
            "Should return NotSleeping"
        );
        assert!(!tab.is_sleeping);
    }

    #[test]
    fn test_can_sleep_regular_tab() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        assert!(tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_pinned_tab() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_pinned = true;
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_internal_page() {
        let dom = Element::new("body");
        let tab = Tab::new(
            1,
            "sylphra://newtab".to_string(),
            dom,
            "New Tab".to_string(),
        );
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_already_sleeping() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.sleep();
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_can_sleep_error_page() {
        let dom = Element::new("body");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Error".to_string(),
        );
        tab.is_error = true;
        assert!(!tab.can_sleep());
    }

    #[test]
    fn test_mark_active_updates_timestamp() {
        let dom = Element::new("body");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let initial_ts = tab.last_active_timestamp;

        std::thread::sleep(std::time::Duration::from_millis(10));
        tab.mark_active();

        assert!(tab.last_active_timestamp >= initial_ts);
    }

    #[test]
    fn test_sleep_wake_cycle_preserves_history() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());

        tab.push_history(HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &dom,
        ));
        tab.push_history(HistoryEntry::new(
            "https://c.com".to_string(),
            "C".to_string(),
            &dom,
        ));
        assert_eq!(tab.history_len(), 3);

        tab.sleep();
        assert!(tab.is_sleeping);

        assert_eq!(tab.history_len(), 3);

        let wake_result = tab.wake();
        assert!(!tab.is_sleeping);
        assert!(
            matches!(wake_result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );

        assert_eq!(tab.history_len(), 3);
    }

    #[test]
    fn test_discarded_tab_has_lowest_score() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let normal_score = tab.discard_score(Some(2));

        tab.discard();
        let discarded_score = tab.discard_score(Some(2));

        assert!(
            discarded_score < normal_score,
            "Discarded tab should have lower score: discarded={} normal={}",
            discarded_score,
            normal_score
        );
    }

    #[test]
    fn test_active_tab_has_very_low_score() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let active_score = tab.discard_score(Some(1));
        let inactive_score = tab.discard_score(Some(2));

        assert!(
            active_score < inactive_score,
            "Active tab should have much lower score: active={} inactive={}",
            active_score,
            inactive_score
        );
        assert!(
            active_score < -5000,
            "Active tab should be heavily protected"
        );
    }

    #[test]
    fn test_pinned_tab_protected() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_pinned = true;

        let pinned_score = tab.discard_score(Some(2));

        let dom2 = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab2 = Tab::new(
            2,
            "https://example.com".to_string(),
            dom2,
            "Test".to_string(),
        );
        tab2.last_active_timestamp -= 600;

        let unpinned_score = tab2.discard_score(Some(3));

        assert!(
            pinned_score < unpinned_score,
            "Pinned tab should be more protected: pinned={} unpinned={}",
            pinned_score,
            unpinned_score
        );
    }

    #[test]
    fn test_audible_tab_protected() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.is_audible = true;

        let audible_score = tab.discard_score(Some(2));

        let dom2 = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab2 = Tab::new(
            2,
            "https://example.com".to_string(),
            dom2,
            "Test".to_string(),
        );
        tab2.last_active_timestamp -= 600;

        let silent_score = tab2.discard_score(Some(3));

        assert!(
            audible_score < silent_score,
            "Audible tab should be more protected: audible={} silent={}",
            audible_score,
            silent_score
        );
    }

    #[test]
    fn test_sleeping_tab_preferred_for_discard() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        tab.last_active_timestamp -= 600;

        let awake_score = tab.discard_score(Some(2));

        tab.sleep();
        let sleeping_score = tab.discard_score(Some(2));

        assert!(
            sleeping_score > awake_score,
            "Sleeping tab should have higher discard score: sleeping={} awake={}",
            sleeping_score,
            awake_score
        );
    }

    #[test]
    fn test_discard_drops_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        assert!(tab.dom.find_tag("h1").is_some());
        assert!(!tab.is_discarded);

        let freed = tab.discard();
        assert!(freed > 0);
        assert!(tab.is_discarded);
        assert!(!tab.is_sleeping);
        assert!(tab.dom.find_tag("h1").is_none());
        assert_eq!(tab.dom.tag, "root");
    }

    #[test]
    fn test_undiscard_restores_url() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.discard();
        assert!(tab.is_discarded);

        let url = tab.undiscard();
        assert_eq!(url, Some("https://example.com".to_string()));
        assert!(!tab.is_discarded);
    }

    #[test]
    fn test_undiscard_noop_when_not_discarded() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let result = tab.undiscard();
        assert!(result.is_none());
    }

    #[test]
    fn test_discard_idempotent() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let freed1 = tab.discard();
        assert!(freed1 > 0);

        let freed2 = tab.discard();
        assert_eq!(freed2, 0, "Second discard should free nothing");
    }

    #[test]
    fn test_scores_sorted_correctly() {
        let dom = crate::parser::parse_html("<html><body><p>Test</p></body></html>");

        let mut tab1 = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());
        tab1.last_active_timestamp -= 1200;

        let mut tab2 = Tab::new(2, "https://b.com".to_string(), dom.clone(), "B".to_string());
        tab2.is_pinned = true;

        let mut tab3 = Tab::new(3, "https://c.com".to_string(), dom.clone(), "C".to_string());
        tab3.is_audible = true;

        let score1 = tab1.discard_score(Some(4));
        let score2 = tab2.discard_score(Some(4));
        let score3 = tab3.discard_score(Some(4));

        assert!(
            score1 > score2,
            "Inactive unpinned should score higher than pinned"
        );
        assert!(
            score1 > score3,
            "Inactive unpinned should score higher than audible"
        );
    }

    #[test]
    fn test_sleep_compresses_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        assert!(tab.compressed_dom.is_none());

        tab.sleep();

        assert!(tab.compressed_dom.is_some(), "Sleep should compress DOM");
        assert!(tab.is_sleeping);
    }

    #[test]
    fn test_wake_restores_from_compressed_dom() {
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        tab.sleep();
        assert!(tab.is_sleeping);
        assert!(tab.compressed_dom.is_some());

        let result = tab.wake();
        assert!(
            matches!(result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!tab.is_sleeping);

        assert_ne!(tab.dom.tag, "root");
        assert!(tab.dom.find_tag("h1").is_some());
        assert!(
            tab.compressed_dom.is_none(),
            "Compressed data should be cleared after wake"
        );
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let dom = crate::parser::parse_html(
            "<html><body><div class=\"main\"><p>Hello</p><p>World</p></div></body></html>",
        );
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let compressed = tab.compress_dom();
        assert!(compressed.is_some());
        let data = compressed.unwrap();
        assert!(!data.is_empty());

        let restored = Tab::decompress_dom(&data);
        assert!(restored.is_ok());

        let restored_dom = restored.unwrap();
        assert_eq!(restored_dom.tag, tab.dom.tag);
        assert_eq!(restored_dom.children.len(), tab.dom.children.len());
    }

    #[test]
    fn test_compress_empty_dom_returns_none() {
        let dom = Element::new("root");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let compressed = tab.compress_dom();
        assert!(
            compressed.is_none(),
            "Empty root DOM should not be compressed"
        );
    }

    #[test]
    fn test_sleep_wake_with_complex_dom() {
        let html = r#"
        <html>
            <head><title>Test Page</title></head>
            <body>
                <header><h1>Welcome</h1></header>
                <main>
                    <p>Paragraph 1</p>
                    <p>Paragraph 2</p>
                    <ul>
                        <li>Item 1</li>
                        <li>Item 2</li>
                        <li>Item 3</li>
                    </ul>
                </main>
                <footer>Footer content</footer>
            </body>
        </html>
        "#;
        let dom = crate::parser::parse_html(html);
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );

        let nodes_before = crate::count_elements(&tab.dom);
        assert!(nodes_before > 5);

        tab.sleep();
        assert!(tab.is_sleeping);
        assert!(tab.compressed_dom.is_some());

        let wake_result = tab.wake();
        assert!(!tab.is_sleeping);
        assert!(
            matches!(wake_result, WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );

        let nodes_after = crate::count_elements(&tab.dom);
        assert_eq!(
            nodes_before, nodes_after,
            "DOM structure should be preserved"
        );
    }

    #[test]
    fn test_history_get_dom_roundtrip_gzip() {
        let html = format!(
            "<html><body>{}</body></html>",
            (0..500)
                .map(|i| format!(
                    "<div class=\"item-{}\" data-idx=\"{}\">Repeated text block number {} with padding padding padding</div>",
                    i, i, i
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let dom = crate::parser::parse_html(&html);
        let entry = HistoryEntry::new("https://example.com".to_string(), "T".to_string(), &dom);

        let restored = entry.get_dom().expect("get_dom must decompress gzip data");
        assert_eq!(restored.tag, dom.tag);
        assert_eq!(restored.children.len(), dom.children.len());
    }

    #[test]
    fn test_history_get_dom_roundtrip_plain_json() {
        let html = "<html><body><h1>Hi</h1></body></html>";
        let dom = crate::parser::parse_html(html);
        let entry = HistoryEntry::new("https://example.com".to_string(), "T".to_string(), &dom);

        let restored = entry.get_dom().expect("plain JSON snapshot must parse");
        assert!(restored.find_tag("h1").is_some());
    }

    #[test]
    fn test_go_back_restores_real_dom_after_gzip() {
        let html = format!(
            "<html><body>{}</body></html>",
            (0..400)
                .map(|i| format!("<p>Item number {} with enough text content</p>", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let dom = crate::parser::parse_html(&html);
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom, "A".to_string());
        let page_b = crate::parser::parse_html("<html><body><h1>Page B</h1></body></html>");
        tab.push_history(HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &page_b,
        ));

        assert!(tab.go_back());
        assert_eq!(tab.url, "https://a.com");
        assert!(
            tab.dom.find_tag("p").is_some() || tab.dom.children.len() > 1,
            "Back must restore the real DOM, got tag={} children={}",
            tab.dom.tag,
            tab.dom.children.len()
        );
    }

    #[test]
    fn test_flatten_deep_caps_depth() {
        let mut deep = Element::new("div");
        for _ in 0..10_000 {
            let mut child = Element::new("div");
            child.text = "x".to_string();
            let old = std::mem::replace(&mut deep, Element::new("div"));
            child.add_child(old);
            deep = child;
        }
        let flattened = flatten_deep(deep, crate::parser::MAX_DOM_DEPTH);

        let mut stack: Vec<(usize, &Element)> = vec![(0, &flattened)];
        let mut max_depth = 0usize;
        while let Some((d, el)) = stack.pop() {
            max_depth = max_depth.max(d);
            for c in &el.children {
                stack.push((d + 1, c));
            }
        }
        assert!(
            max_depth <= crate::parser::MAX_DOM_DEPTH,
            "flatten_deep left depth {} (cap {})",
            max_depth,
            crate::parser::MAX_DOM_DEPTH
        );
    }
}
