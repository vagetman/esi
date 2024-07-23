/// This struct is used to configure optional behaviour within the ESI processor.
///
/// ## Usage Example
/// ```rust,no_run
/// let config = esi::Configuration::default()
///     .with_namespace("app");
/// ```
#[allow(clippy::return_self_not_must_use)]
#[derive(Clone, Debug)]
pub struct Configuration {
    /// The XML namespace to use when scanning for ESI tags. Defaults to `esi`.
    pub namespace: String,
    /// For working with non-HTML ESI templates, e.g. JSON files, this option allows you to disable the unescaping of URLs
    pub is_escaped_content: bool,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            namespace: String::from("esi"),
            is_escaped_content: true,
        }
    }
}

impl Configuration {
    /// Sets an alternative ESI namespace, which is used to identify ESI instructions.
    ///
    /// For example, setting this to `test` would cause the processor to only match tags like `<test:include>`.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }
    /// For working with non-HTML ESI templates, eg JSON files, allows to disable URLs unescaping
    pub fn with_escaped(mut self, is_escaped: impl Into<bool>) -> Self {
        self.is_escaped_content = is_escaped.into();
        self
    }
}
