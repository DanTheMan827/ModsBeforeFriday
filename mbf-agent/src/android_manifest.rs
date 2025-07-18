#[cfg(feature = "cli")]
use std::collections::HashMap;

#[cfg(feature = "cli")]
use xmltree::{Element, XMLNode};

#[cfg(feature = "cli")]
const ANDROID_NS_URI: &str = "http://schemas.android.com/apk/res/android";

#[cfg(feature = "cli")]
#[derive(Debug)]
pub struct AndroidManifest {
    features: Vec<String>,
    permissions: Vec<String>,
    native_libraries: Vec<String>,
    metadata: HashMap<String, String>,
    document: Element,
    android_ns_prefix: String,
}



#[cfg(feature = "cli")]
impl AndroidManifest {
    fn manifest_el(&mut self) -> &Element {
       &self.document
    }
    
    fn application_el(&mut self) -> &mut Element {
        self.document
            .get_mut_child("application")
            .expect("Missing <application> tag")
    }
    
    pub fn new(manifest_xml: &str) -> Result<Self, String> {
        let document = Element::parse(manifest_xml.as_bytes())
            .map_err(|e| format!("Invalid XML: {}", e))?;

        let android_ns_prefix = "android".into(); // Simplification: hardcoded ns prefix
        
        // Initialize empty variables
        let mut features: Vec<String> = Vec::new();
        let mut permissions: Vec<String> = Vec::new();
        let mut native_libraries: Vec<String> = Vec::new();
        let mut metadata: HashMap<String, String> = HashMap::new();

        for child in document.children.iter() {
            if let XMLNode::Element(el) = child {
                match el.name.as_str() {
                    "uses-permission" => {
                        if let Some(name) = el.attributes.get("android:name") {
                            permissions.push(name.clone());
                        }
                    }
                    "uses-feature" => {
                        if let Some(name) = el.attributes.get("android:name") {
                            features.push(name.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        
        let application_el = document
            .get_child("application")
            .ok_or("Missing <application> tag")
            .unwrap();

        for child in &application_el.children {
            if let XMLNode::Element(el) = child {
                if el.name == "uses-native-library" {
                    if let Some(name) = el.attributes.get("android:name") {
                        native_libraries.push(name.clone());
                    }
                } else if el.name == "meta-data" {
                    if let (Some(name), Some(value)) = (
                        el.attributes.get("android:name"),
                        el.attributes.get("android:value"),
                    ) {
                        metadata.insert(name.clone(), value.clone());
                    }
                }
            }
        }

        let mut manifest = AndroidManifest {
            features,
            permissions,
            native_libraries,
            metadata,
            document,
            android_ns_prefix,
        };
        
        Ok(manifest)
    }

    pub fn to_string(&self) -> String {
        let mut buf = Vec::new();
        self.document.write(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    pub fn get_permissions(&self) -> &[String] {
        &self.permissions
    }

    pub fn get_features(&self) -> &[String] {
        &self.features
    }

    pub fn get_metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    pub fn get_native_libraries(&self) -> &[String] {
        &self.native_libraries
    }
    
    pub fn has_permission(&self, perm: &str) -> bool {
        return self.permissions.contains(&perm.to_string());
    }
    
    pub fn has_feature(&self, feat: &str) -> bool {
        return self.features.contains(&feat.to_string());
    }
    
    pub fn has_metadata(&self, name: &str) -> bool {
        return self.metadata.contains_key(name);
    }
    
    pub fn has_native_library(&self, file_name: &str) -> bool {
        return self.native_libraries.contains(&file_name.to_string());
    }

    pub fn apply_patching_manifest_mod(&mut self) {
        let application_el = self.application_el();
        
        application_el
            .attributes
            .insert("android:debuggable".into(), "true".into());
        application_el
            .attributes
            .insert("android:hardwareAccelerated".into(), "true".into());

        self.add_permission("android.permission.MANAGE_EXTERNAL_STORAGE");
    }

    pub fn add_permission(&mut self, perm: &str) {
        if self.has_permission(perm) {
            return;
        }

        let mut el = Element::new("uses-permission");
        el.attributes.insert("android:name".into(), perm.to_string());
        self.document.children.push(XMLNode::Element(el));
        self.permissions.push(perm.to_string());
    }

    pub fn add_feature(&mut self, feat: &str) {
        if self.has_feature(feat) {
            return;
        }

        let mut el = Element::new("uses-feature");
        el.attributes.insert("android:name".into(), feat.to_string());
        el.attributes.insert("android:required".into(), "false".into());
        self.document.children.push(XMLNode::Element(el));
        self.features.push(feat.to_string());
    }
    
    pub fn add_native_library(&mut self, file_name: &str) {
        if self.has_native_library(file_name) {
            return;
        }

        let application_el = self.application_el();
        let mut el = Element::new("uses-native-library");
        el.attributes.insert("android:name".into(), file_name.to_string());
        el.attributes.insert("android:required".into(), "false".into());
        application_el.children.push(XMLNode::Element(el));
        self.native_libraries.push(file_name.to_string());
    }

    pub fn set_metadata(&mut self, name: &str, value: &str) {
        let application_el = self.application_el();
        for child in application_el.children.iter_mut() {
            if let XMLNode::Element(el) = child {
                if el.name == "meta-data" {
                    if let Some(attr_name) = el.attributes.get("android:name") {
                        if attr_name == name {
                            el.attributes
                                .insert("android:value".into(), value.to_string());
                            self.metadata.insert(name.to_string(), value.to_string());
                            return;
                        }
                    }
                }
            }
        }

        let mut new_el = Element::new("meta-data");
        new_el.attributes.insert("android:name".into(), name.to_string());
        new_el.attributes.insert("android:value".into(), value.to_string());
        application_el.children.push(XMLNode::Element(new_el));
        self.metadata.insert(name.to_string(), value.to_string());
    }

    pub fn remove_metadata(&mut self, name: &str) {
        self.application_el().children.retain(|child| {
            if let XMLNode::Element(el) = child {
                !(el.name == "meta-data"
                    && el.attributes.get("android:name") == Some(&name.to_string()))
            } else {
                true
            }
        });
        self.metadata.remove(name);
    }

    pub fn remove_permission(&mut self, perm: &str) {
        self.document.children.retain(|child| {
            if let XMLNode::Element(el) = child {
                !(el.name == "uses-permission"
                    && el.attributes.get("android:name") == Some(&perm.to_string()))
            } else {
                true
            }
        });
        self.permissions.retain(|p| p != perm);
    }

    pub fn remove_feature(&mut self, feat: &str) {
        self.document.children.retain(|child| {
            if let XMLNode::Element(el) = child {
                !(el.name == "uses-feature"
                    && el.attributes.get("android:name") == Some(&feat.to_string()))
            } else {
                true
            }
        });
        self.features.retain(|f| f != feat);
    }

    pub fn remove_native_library(&mut self, file_name: &str) {
        self.application_el().children.retain(|child| {
            if let XMLNode::Element(el) = child {
                !(el.name == "uses-native-library"
                    && el.attributes.get("android:name") == Some(&file_name.to_string()))
            } else {
                true
            }
        });
        self.native_libraries.retain(|lib| lib != file_name);
    }
}
