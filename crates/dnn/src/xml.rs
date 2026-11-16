//! Changing a site's XML file (`SiteXml.Load` + `XDocument` + `XmlWriter` in the C#): read as untrusted input - a DTD
//! refused (`web_config::load_site_xml`) - changed as a tree, and written back with its XML declaration and line
//! endings as they were.

use dnnm_infrastructure::web_config::load_site_xml;
use xot::{Node, Xot};

/// A site XML file loaded to be changed.
pub struct SiteXml {
    pub xot: Xot,
    pub document: Node,
    declaration: Option<String>,
    crlf: bool,
}

impl SiteXml {
    /// Parses `text`; Err with the parser's message, or when it has a DTD.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        load_site_xml(text)?;
        let mut xot = Xot::new();
        let document = xot.parse(text).map_err(|e| e.to_string())?;
        let trimmed = text.trim_start();
        let declaration = trimmed
            .starts_with("<?xml")
            .then(|| trimmed.find("?>").map(|end| trimmed[..end + 2].to_string()))
            .flatten();
        Ok(Self { xot, document, declaration, crlf: text.contains("\r\n") })
    }

    pub fn root(&self) -> Option<Node> {
        self.xot.document_element(self.document).ok()
    }

    /// The document as text: its declaration kept when it had one, and its line endings.
    pub fn to_text(&self) -> Result<String, String> {
        let body = self.xot.to_string(self.document).map_err(|e| e.to_string())?;
        let mut text = match &self.declaration {
            Some(declaration) => {
                format!("{declaration}{}{body}", if body.starts_with(['\r', '\n']) { "" } else { "\r\n" })
            }
            None => body,
        };
        text = text.replace("\r\n", "\n");
        if self.crlf || self.declaration.is_some() {
            text = text.replace('\n', "\r\n");
        }
        Ok(text)
    }

    /// The element's local name and namespace URI ("" for none).
    pub fn name_of(&self, node: Node) -> Option<(&str, &str)> {
        let element = self.xot.element(node)?;
        Some((self.xot.local_name_str(element.name()), self.xot.uri_str(element.name())))
    }

    /// The child elements of `parent` named `local` in `namespace` ("" for none) - `XElement.Elements(name)`.
    pub fn elements(&self, parent: Node, local: &str, namespace: &str) -> Vec<Node> {
        self.xot.children(parent).filter(|c| self.name_of(*c) == Some((local, namespace))).collect()
    }

    pub fn element(&self, parent: Node, local: &str, namespace: &str) -> Option<Node> {
        self.elements(parent, local, namespace).into_iter().next()
    }

    pub fn attribute(&self, node: Node, name: &str) -> Option<&str> {
        let id = self.xot.name(name)?;
        self.xot.get_attribute(node, id)
    }

    pub fn set_attribute(&mut self, node: Node, name: &str, value: &str) {
        let id = self.xot.add_name(name);
        self.xot.set_attribute(node, id, value);
    }

    /// A new element in `namespace` ("" for none).
    pub fn new_element(&mut self, local: &str, namespace: &str) -> Node {
        let name = if namespace.is_empty() {
            self.xot.add_name(local)
        } else {
            let ns = self.xot.add_namespace(namespace);
            self.xot.add_name_ns(local, ns)
        };
        self.xot.new_element(name)
    }

    /// Appends a new element to `parent` and returns it.
    pub fn append_element(&mut self, parent: Node, local: &str, namespace: &str) -> Result<Node, String> {
        let node = self.new_element(local, namespace);
        self.xot.append(parent, node).map_err(|e| e.to_string())?;
        Ok(node)
    }

    /// The text of an element - `XElement.Value` for one that holds only text.
    pub fn text(&self, node: Node) -> String {
        self.xot.text_content_str(node).map(str::to_string).unwrap_or_default()
    }

    /// Replaces what an element holds with `value` (`XElement.Value = …`).
    pub fn set_text(&mut self, node: Node, value: &str) -> Result<(), String> {
        let children: Vec<Node> = self.xot.children(node).collect();
        for child in children {
            self.xot.remove(child).map_err(|e| e.to_string())?;
        }
        if !value.is_empty() {
            self.xot.append_text(node, value).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
