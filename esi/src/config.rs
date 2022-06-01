/// This struct is used to configure optional behaviour within the ESI processor.
///
/// ## Usage Example
/// ```rust,no_run
/// let config = esi::Configuration::default()
///     .with_namespace("app")
///     .with_recursion();
///
/// let processor = esi::Processor::new(config);
/// ```
#[derive(Clone, Debug)]
pub struct Configuration {
    /// The XML namespace to use when scanning for ESI tags. Defaults to `esi`.
    pub namespace: String,

    /// Whether or not to execute nested ESI tags within fetched fragments. Defaults to `false`.
    pub recursive: bool,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            namespace: String::from("esi"),
            recursive: false,
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

    /// Enables the processing of nested ESI tags within fetched fragments.
    pub fn with_recursion(mut self) -> Self {
        self.recursive = true;
        self
    }
}
