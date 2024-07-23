#[derive(Debug)]
pub struct EsiData<'e> {
    elements: Vec<TextOrFunction<'e>>,
}

#[derive(Debug)]
pub enum TextOrFunction<'e> {
    Text(&'e str),
    Function(Function),
}
#[derive(Debug)]
pub struct Function {
    name: String,
    args: Vec<Arg>,
}

#[derive(Debug)]
pub enum Arg {
    Function(Function),
    Text(String),
}

impl<'e> EsiData<'e> {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
        }
    }
    pub fn from_text(text: &'e str) -> Self {
        Self {
            elements: vec![TextOrFunction::Text(text)],
        }
    }
    // pub fn add_text(&mut self, text: &'e str) {
    //     self.elements.push(TextOrFunction::Text(text));
    // }
    // pub fn add_function(&mut self, function: Function) {
    //     self.elements.push(TextOrFunction::Function(function));
    // }
}
