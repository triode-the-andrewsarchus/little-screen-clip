//! Parser for the Structurizr DSL.
//!
//! Parses tokens into an AST and then converts to a Workspace.
//!
//! This extraction parses self-contained memory inputs and never executes scripts.

use std::collections::{HashMap, HashSet};

use structurizr_core::prelude::*;

use crate::ast::*;
use crate::error::{Error, ParseError, Result};
use crate::lexer::{Lexer, Token, TokenKind};

/// Parse a self-contained workspace without filesystem, network or execution capabilities.
///
/// Includes, workspace extension, scripts and plugins fail rather than becoming
/// silently incomplete previews. Documentation paths remain inert model metadata.
pub fn parse(input: &str) -> Result<Workspace> {
    let ast = parse_to_ast(input)?;
    if ast.extends.is_some()
        || ast.directives.iter().any(|directive| {
            matches!(
                directive,
                Directive::Include(_)
                    | Directive::Extend(_)
                    | Directive::Script(_)
                    | Directive::Plugin(_)
            )
        })
    {
        return Err(Error::Parse(ParseError {
            message: "Preview requires a self-contained workspace; includes, extends, scripts and plugins are not loaded".into(),
            location: None,
        }));
    }
    build_workspace(ast)
}

/// Parse a DSL string into an AST (for testing/debugging).
pub fn parse_to_ast(input: &str) -> Result<WorkspaceNode> {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize()?;
    // Use lexer tokens rather than source characters: braces in labels and
    // comments are data. Bound recursion before recursive descent begins.
    const MAX_DEPTH: usize = 64;
    let mut braces = Vec::new();
    for token in &tokens {
        match token.kind {
            TokenKind::OpenBrace => {
                if braces.len() == MAX_DEPTH {
                    return Err(Error::Lexer {
                        message: "Nesting exceeds the preview depth budget".into(),
                        location: token.location,
                    });
                }
                braces.push(token.location);
            }
            TokenKind::CloseBrace => {
                if braces.pop().is_none() {
                    return Err(Error::Lexer {
                        message: "Unexpected closing brace".into(),
                        location: token.location,
                    });
                }
            }
            TokenKind::Eof => {
                if let Some(location) = braces.pop() {
                    return Err(Error::Lexer {
                        message: "Unclosed block".into(),
                        location,
                    });
                }
            }
            _ => {}
        }
    }
    let mut parser = Parser::with_source(tokens, input.to_string());
    let workspace = parser.parse_workspace()?;
    parser.skip_newlines_and_comments();
    if !matches!(parser.current_kind(), Some(TokenKind::Eof) | None) {
        return Err(Error::Parse(ParseError {
            message: "Unexpected content after workspace".into(),
            location: parser.current().map(|token| token.location),
        }));
    }
    Ok(workspace)
}

/// Parser state.
pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    /// Original source text (reserved for future raw script extraction)
    #[allow(dead_code)]
    source: String,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
            source: String::new(),
        }
    }

    /// Create a parser with source text for script extraction.
    pub fn with_source(tokens: Vec<Token>, source: String) -> Self {
        Self {
            tokens,
            position: 0,
            source,
        }
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn current_kind(&self) -> Option<&TokenKind> {
        self.current().map(|t| &t.kind)
    }

    fn advance(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.position);
        self.position += 1;
        token
    }

    fn skip_newlines_and_comments(&mut self) {
        while let Some(kind) = self.current_kind() {
            match kind {
                TokenKind::Newline | TokenKind::Comment => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn expect(&mut self, expected: TokenKind) -> Result<&Token> {
        self.skip_newlines_and_comments();
        match self.current() {
            Some(token) if std::mem::discriminant(&token.kind) == std::mem::discriminant(&expected) => {
                self.position += 1;
                Ok(&self.tokens[self.position - 1])
            }
            Some(token) => Err(Error::Parse(ParseError {
                message: format!("Expected {:?}, found {:?}", expected, token.kind),
                location: Some(token.location),
            })),
            None => Err(Error::Parse(ParseError {
                message: format!("Expected {:?}, found end of input", expected),
                location: None,
            })),
        }
    }

    fn expect_string(&mut self) -> Result<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::String(s)) => {
                self.advance();
                Ok(s)
            }
            Some(kind) => {
                let loc = self.current().map(|t| t.location);
                Err(Error::Parse(ParseError {
                    message: format!("Expected string, found {:?}", kind),
                    location: loc,
                }))
            }
            None => Err(Error::Parse(ParseError {
                message: "Expected string, found end of input".to_string(),
                location: None,
            })),
        }
    }

    fn expect_identifier(&mut self) -> Result<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::Identifier(s)) => {
                self.advance();
                Ok(s)
            }
            Some(kind) => {
                let loc = self.current().map(|t| t.location);
                Err(Error::Parse(ParseError {
                    message: format!("Expected identifier, found {:?}", kind),
                    location: loc,
                }))
            }
            None => Err(Error::Parse(ParseError {
                message: "Expected identifier, found end of input".to_string(),
                location: None,
            })),
        }
    }

    /// Expects an identifier or a keyword that can be used as a shape name (e.g., Person).
    fn expect_identifier_or_shape_keyword(&mut self) -> Result<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::Identifier(s)) => {
                self.advance();
                Ok(s)
            }
            // Shape keywords that are also DSL keywords
            Some(TokenKind::Person) => {
                self.advance();
                Ok("Person".to_string())
            }
            Some(TokenKind::Component) => {
                self.advance();
                Ok("Component".to_string())
            }
            Some(kind) => {
                let loc = self.current().map(|t| t.location);
                Err(Error::Parse(ParseError {
                    message: format!("Expected shape name, found {:?}", kind),
                    location: loc,
                }))
            }
            None => Err(Error::Parse(ParseError {
                message: "Expected shape name, found end of input".to_string(),
                location: None,
            })),
        }
    }

    fn maybe_string(&mut self) -> Option<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::String(s)) => {
                self.advance();
                Some(s)
            }
            _ => None,
        }
    }

    /// Parse an optional string or number (used for instances which can be "1", "2-10", or just 5)
    fn maybe_string_or_number(&mut self) -> Option<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::String(s)) => {
                self.advance();
                Some(s)
            }
            Some(TokenKind::Number(n)) => {
                self.advance();
                Some(n.to_string())
            }
            _ => None,
        }
    }

    /// Expects an identifier or a string (for !ref which can use either)
    fn expect_identifier_or_string(&mut self) -> Result<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::Identifier(s)) => {
                self.advance();
                Ok(s)
            }
            Some(TokenKind::String(s)) => {
                self.advance();
                Ok(s)
            }
            Some(kind) => {
                let loc = self.current().map(|t| t.location);
                Err(Error::Parse(ParseError {
                    message: format!("Expected identifier or string, found {:?}", kind),
                    location: loc,
                }))
            }
            None => Err(Error::Parse(ParseError {
                message: "Expected identifier or string, found end of input".to_string(),
                location: None,
            })),
        }
    }

    /// Expects an identifier or a keyword that can be used as a configuration value
    /// (e.g., "softwaresystem", "landscape", "none", "public", "private")
    fn expect_identifier_or_keyword(&mut self) -> Result<String> {
        self.skip_newlines_and_comments();
        match self.current_kind().cloned() {
            Some(TokenKind::Identifier(s)) => {
                self.advance();
                Ok(s)
            }
            // Keywords that can be used as configuration values
            Some(TokenKind::SoftwareSystem) => {
                self.advance();
                Ok("softwaresystem".to_string())
            }
            Some(TokenKind::SystemLandscape) => {
                self.advance();
                Ok("landscape".to_string())
            }
            Some(kind) => {
                let loc = self.current().map(|t| t.location);
                Err(Error::Parse(ParseError {
                    message: format!("Expected identifier or keyword, found {:?}", kind),
                    location: loc,
                }))
            }
            None => Err(Error::Parse(ParseError {
                message: "Expected identifier or keyword, found end of input".to_string(),
                location: None,
            })),
        }
    }

    /// Skip a block (including nested blocks) - used for unimplemented features
    fn skip_block(&mut self) -> Result<()> {
        self.skip_newlines_and_comments();
        if !self.check(&TokenKind::OpenBrace) {
            return Ok(());
        }
        self.advance(); // consume {
        let mut depth = 1;
        while depth > 0 {
            match self.current_kind() {
                Some(TokenKind::OpenBrace) => {
                    depth += 1;
                    self.advance();
                }
                Some(TokenKind::CloseBrace) => {
                    depth -= 1;
                    self.advance();
                }
                Some(TokenKind::Eof) | None => {
                    return Err(Error::Parse(ParseError {
                        message: "Unexpected end of input while skipping block".to_string(),
                        location: self.current().map(|t| t.location),
                    }));
                }
                _ => {
                    self.advance();
                }
            }
        }
        Ok(())
    }

    fn check(&self, expected: &TokenKind) -> bool {
        match self.current_kind() {
            Some(kind) => std::mem::discriminant(kind) == std::mem::discriminant(expected),
            None => false,
        }
    }

    pub fn parse_workspace(&mut self) -> Result<WorkspaceNode> {
        self.skip_newlines_and_comments();

        // Check for optional "workspace" keyword
        let has_workspace_keyword = matches!(self.current_kind(), Some(TokenKind::Workspace));
        if has_workspace_keyword {
            self.advance();
        }

        // Optional name and description
        let name = self.maybe_string();
        let description = self.maybe_string();

        // Check for "extends"
        let extends = if matches!(self.current_kind(), Some(TokenKind::Extends)) {
            self.advance();
            Some(self.expect_string()?)
        } else {
            None
        };

        self.skip_newlines_and_comments();

        // Opening brace (optional for top-level)
        let has_brace = self.check(&TokenKind::OpenBrace);
        if has_brace {
            self.advance();
        }

        let mut workspace = WorkspaceNode {
            name,
            description,
            extends,
            model: None,
            views: None,
            properties: HashMap::new(),
            directives: Vec::new(),
            configuration: None,
        };

        // Parse workspace contents
        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) if has_brace => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Eof) => break,
                Some(TokenKind::Model) => {
                    self.advance();
                    workspace.model = Some(self.parse_model()?);
                }
                Some(TokenKind::Views) => {
                    self.advance();
                    workspace.views = Some(self.parse_views()?);
                }
                Some(TokenKind::Properties) => {
                    self.advance();
                    workspace.properties = self.parse_properties_block()?;
                }
                Some(TokenKind::DirectiveIdentifiers) => {
                    self.advance();
                    let scope = self.expect_identifier()?;
                    let scope = if scope.to_lowercase() == "hierarchical" {
                        IdentifierScope::Hierarchical
                    } else {
                        IdentifierScope::Flat
                    };
                    workspace.directives.push(Directive::Identifiers(scope));
                }
                Some(TokenKind::DirectiveImpliedRelationships) => {
                    self.advance();
                    let location = self.current().map(|token| token.location);
                    let strategy = self.expect_identifier()?;
                    let enabled = match strategy.to_ascii_lowercase().as_str() {
                        "true" | "on" => true,
                        "false" | "off" => false,
                        _ => return Err(Error::Parse(ParseError {
                            message: "Unsupported implied relationship strategy; use true or false".into(),
                            location,
                        })),
                    };
                    workspace.directives.push(Directive::ImpliedRelationships(enabled));
                }
                Some(TokenKind::DirectiveConst) => {
                    self.advance();
                    let name = self.expect_identifier()?;
                    let value = self.expect_string()?;
                    workspace.directives.push(Directive::Const { name, value });
                }
                Some(TokenKind::DirectiveInclude) => {
                    self.advance();
                    let path = self.expect_string()?;
                    workspace.directives.push(Directive::Include(path));
                }
                Some(TokenKind::DirectiveDocs) => {
                    self.advance();
                    let path = self.expect_string()?;
                    workspace.directives.push(Directive::Docs(path));
                }
                Some(TokenKind::DirectiveAdrs) => {
                    self.advance();
                    let path = self.expect_string()?;
                    workspace.directives.push(Directive::Adrs(path));
                }
                Some(TokenKind::DirectiveRef) => {
                    self.advance();
                    let identifier = self.expect_identifier_or_string()?;
                    workspace.directives.push(Directive::Ref(identifier));
                }
                Some(TokenKind::DirectiveExtend) => {
                    self.advance();
                    let path = self.expect_string()?;
                    workspace.directives.push(Directive::Extend(path));
                }
                Some(TokenKind::DirectivePlugin) => {
                    // !plugin <classname> - stored but not executed (requires JVM)
                    self.advance();
                    let class_name = self.expect_identifier_or_string()?;
                    workspace.directives.push(Directive::Plugin(class_name));
                    // Skip any block if present
                    if self.check(&TokenKind::OpenBrace) {
                        self.skip_block()?;
                    }
                }
                Some(TokenKind::DirectiveScript) => {
                    // !script groovy { ... } - inline script with language
                    // !script "path/to/script.groovy" - external script file
                    self.advance();
                    let script = self.parse_script_directive()?;
                    workspace.directives.push(Directive::Script(script));
                }
                Some(TokenKind::Configuration) => {
                    self.advance();
                    workspace.configuration = Some(self.parse_configuration()?);
                }
                Some(kind) => {
                    return Err(Error::Parse(ParseError {
                        message: format!("Unexpected token in workspace: {:?}", kind),
                        location: self.current().map(|t| t.location),
                    }));
                }
                None => break,
            }
        }

        Ok(workspace)
    }

    fn parse_model(&mut self) -> Result<ModelNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut model = ModelNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Person) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::Person, None)?;
                    model.elements.push(element);
                }
                Some(TokenKind::SoftwareSystem) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::SoftwareSystem, None)?;
                    model.elements.push(element);
                }
                Some(TokenKind::DeploymentNode) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::DeploymentNode, None)?;
                    model.elements.push(element);
                }
                Some(TokenKind::DeploymentEnvironment) => {
                    self.advance();
                    let env = self.parse_deployment_environment()?;
                    model.deployment_environments.push(env);
                }
                Some(TokenKind::Group) => {
                    self.advance();
                    let group = self.parse_group()?;
                    model.groups.push(group);
                }
                Some(TokenKind::Enterprise) => {
                    self.advance();
                    let enterprise = self.parse_enterprise()?;
                    model.enterprise = Some(enterprise);
                }
                Some(TokenKind::CustomElement | TokenKind::Element) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::CustomElement, None)?;
                    model.elements.push(element);
                }
                Some(TokenKind::DirectiveRef) => {
                    self.advance();
                    // !ref in model context - just skip the identifier for now
                    let _identifier = self.expect_identifier_or_string()?;
                    // In full implementation, this would resolve and bring the element into scope
                }
                Some(TokenKind::Identifier(id)) => {
                    // Could be: identifier = element, or identifier -> target
                    let id = id.clone();
                    self.advance();

                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Equals) {
                        self.advance();
                        self.skip_newlines_and_comments();

                        match self.current_kind() {
                            Some(TokenKind::Person) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::Person, Some(id))?;
                                model.elements.push(element);
                            }
                            Some(TokenKind::SoftwareSystem) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::SoftwareSystem, Some(id))?;
                                model.elements.push(element);
                            }
                            Some(TokenKind::DeploymentNode) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::DeploymentNode, Some(id))?;
                                model.elements.push(element);
                            }
                            Some(TokenKind::CustomElement | TokenKind::Element) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::CustomElement, Some(id))?;
                                model.elements.push(element);
                            }
                            _ => {
                                return Err(Error::Parse(ParseError {
                                    message: "Expected element type after '='".to_string(),
                                    location: self.current().map(|t| t.location),
                                }));
                            }
                        }
                    } else if self.check(&TokenKind::Arrow) {
                        self.advance();
                        let rel = self.parse_relationship(id)?;
                        model.relationships.push(rel);
                    } else {
                        return Err(Error::Parse(ParseError {
                            message: "Expected '=' or '->' after identifier".to_string(),
                            location: self.current().map(|t| t.location),
                        }));
                    }
                }
                Some(kind) => {
                    return Err(Error::Parse(ParseError {
                        message: format!("Unexpected token in model: {:?}", kind),
                        location: self.current().map(|t| t.location),
                    }));
                }
                None => {
                    return Err(Error::Parse(ParseError {
                        message: "Unexpected end of input in model".to_string(),
                        location: None,
                    }));
                }
            }
        }

        Ok(model)
    }

    fn parse_element(&mut self, kind: ElementKind, identifier: Option<String>) -> Result<ElementNode> {
        self.skip_newlines_and_comments();

        // ContainerInstance and SoftwareSystemInstance use an identifier reference, not a string
        let name = if matches!(kind, ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance) {
            self.expect_identifier()?
        } else {
            self.expect_string()?
        };

        // For custom elements: name, optional metadata, description and tags
        // For others: name, description, technology (optional)
        let (description, technology) = if matches!(kind, ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance) {
            let groups = self.maybe_string().unwrap_or_default();
            if !groups.is_empty() {
                return Err(Error::Parse(ParseError {
                    message: "Deployment groups are not yet supported by the native preview".into(),
                    location: self.current().map(|token| token.location),
                }));
            }
            (None, None)
        } else if kind == ElementKind::CustomElement {
            let metadata = self.maybe_string();
            let desc = self.maybe_string(); // description is optional
            (desc, metadata)
        } else {
            let desc = self.maybe_string();
            // Technology is the 3rd string for Container/Component/DeploymentNode
            let tech = if matches!(kind, ElementKind::Container | ElementKind::Component | ElementKind::DeploymentNode | ElementKind::InfrastructureNode) {
                self.maybe_string()
            } else {
                None
            };
            (desc, tech)
        };

        // For DeploymentNode: parse inline tags (4th) and instances (5th)
        // Syntax: deploymentNode <name> [description] [technology] [tags] [instances] { ... }
        let (inline_tags, instances) = if matches!(kind, ElementKind::DeploymentNode | ElementKind::InfrastructureNode | ElementKind::CustomElement | ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance) {
            let tags_str = self.maybe_string();
            let instances_str = if kind == ElementKind::DeploymentNode {
                self.maybe_string_or_number()
            } else {
                None
            };
            (tags_str, instances_str)
        } else {
            (None, None)
        };

        let mut element = ElementNode {
            identifier,
            kind,
            name,
            description,
            technology,
            tags: inline_tags
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default(),
            properties: HashMap::new(),
            children: Vec::new(),
            relationships: Vec::new(),
            instances,
            health_checks: Vec::new(),
        };

        // Check for optional block
        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();
            self.parse_element_body(&mut element)?;
        }

        Ok(element)
    }

    fn parse_element_body(&mut self, element: &mut ElementNode) -> Result<()> {
        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Tags) => {
                    self.advance();
                    let tags = self.expect_string()?;
                    element.tags = tags.split(',').map(|s| s.trim().to_string()).collect();
                }
                Some(TokenKind::Technology) => {
                    self.advance();
                    element.technology = Some(self.expect_string()?);
                }
                Some(TokenKind::Description) => {
                    self.advance();
                    element.description = Some(self.expect_string()?);
                }
                Some(TokenKind::Url) => {
                    self.advance();
                    let url = self.expect_string()?;
                    element.properties.insert("url".to_string(), url);
                }
                Some(TokenKind::Instances) if element.kind == ElementKind::DeploymentNode => {
                    self.advance();
                    // instances can be a number or string (e.g., "2-10")
                    let instances = self.maybe_string_or_number().ok_or_else(|| {
                        Error::Parse(ParseError {
                            message: "Expected number or string after 'instances'".to_string(),
                            location: self.current().map(|t| t.location),
                        })
                    })?;
                    element.instances = Some(instances);
                }
                Some(TokenKind::Properties) => {
                    self.advance();
                    element.properties.extend(self.parse_properties_block()?);
                }
                // Health check for container/software system instances
                Some(TokenKind::HealthCheck) if matches!(element.kind, ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance) => {
                    self.advance();
                    let health_check = self.parse_health_check()?;
                    element.health_checks.push(health_check);
                }
                // Child elements
                Some(TokenKind::Container) if element.kind == ElementKind::SoftwareSystem => {
                    self.advance();
                    let child = self.parse_element(ElementKind::Container, None)?;
                    element.children.push(child);
                }
                Some(TokenKind::Component) if element.kind == ElementKind::Container => {
                    self.advance();
                    let child = self.parse_element(ElementKind::Component, None)?;
                    element.children.push(child);
                }
                Some(TokenKind::DeploymentNode) if element.kind == ElementKind::DeploymentNode => {
                    self.advance();
                    let child = self.parse_element(ElementKind::DeploymentNode, None)?;
                    element.children.push(child);
                }
                Some(TokenKind::InfrastructureNode) if element.kind == ElementKind::DeploymentNode => {
                    self.advance();
                    let child = self.parse_element(ElementKind::InfrastructureNode, None)?;
                    element.children.push(child);
                }
                Some(TokenKind::ContainerInstance) if element.kind == ElementKind::DeploymentNode => {
                    self.advance();
                    let child = self.parse_element(ElementKind::ContainerInstance, None)?;
                    element.children.push(child);
                }
                Some(TokenKind::SoftwareSystemInstance) if element.kind == ElementKind::DeploymentNode => {
                    self.advance();
                    let child = self.parse_element(ElementKind::SoftwareSystemInstance, None)?;
                    element.children.push(child);
                }
                // Identifier could be child element with identifier or relationship
                Some(TokenKind::Identifier(id)) => {
                    let id = id.clone();
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Equals) {
                        self.advance();
                        self.skip_newlines_and_comments();

                        match (self.current_kind(), element.kind) {
                            (Some(TokenKind::Container), ElementKind::SoftwareSystem) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::Container, Some(id))?;
                                element.children.push(child);
                            }
                            (Some(TokenKind::Component), ElementKind::Container) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::Component, Some(id))?;
                                element.children.push(child);
                            }
                            (Some(TokenKind::DeploymentNode), ElementKind::DeploymentNode) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::DeploymentNode, Some(id))?;
                                element.children.push(child);
                            }
                            (Some(TokenKind::ContainerInstance), ElementKind::DeploymentNode) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::ContainerInstance, Some(id))?;
                                element.children.push(child);
                            }
                            (Some(TokenKind::SoftwareSystemInstance), ElementKind::DeploymentNode) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::SoftwareSystemInstance, Some(id))?;
                                element.children.push(child);
                            }
                            (Some(TokenKind::InfrastructureNode), ElementKind::DeploymentNode) => {
                                self.advance();
                                let child = self.parse_element(ElementKind::InfrastructureNode, Some(id))?;
                                element.children.push(child);
                            }
                            _ => {
                                return Err(Error::Parse(ParseError {
                                    message: "Unexpected element type in body".to_string(),
                                    location: self.current().map(|t| t.location),
                                }));
                            }
                        }
                    } else if self.check(&TokenKind::Arrow) {
                        self.advance();
                        let rel = self.parse_relationship(id)?;
                        element.relationships.push(rel);
                    }
                }
                Some(kind) => {
                    return Err(Error::Parse(ParseError {
                        message: format!("Unexpected token in element body: {:?}", kind),
                        location: self.current().map(|t| t.location),
                    }));
                }
                None => {
                    return Err(Error::Parse(ParseError {
                        message: "Unexpected end of input in element body".to_string(),
                        location: None,
                    }));
                }
            }
        }

        Ok(())
    }

    /// Parse a health check definition.
    /// Syntax: healthCheck <name> <url> [interval] [timeout]
    fn parse_health_check(&mut self) -> Result<HealthCheckNode> {
        self.skip_newlines_and_comments();

        let name = self.expect_string()?;
        let url = self.expect_string()?;

        // Optional interval (in seconds) and timeout (in milliseconds)
        let interval = self.maybe_string_or_number().and_then(|s| s.parse::<u32>().ok());
        let timeout = self.maybe_string_or_number().and_then(|s| s.parse::<u32>().ok());

        Ok(HealthCheckNode {
            name,
            url,
            interval,
            timeout,
        })
    }

    fn parse_relationship(&mut self, source: String) -> Result<RelationshipNode> {
        self.skip_newlines_and_comments();

        let destination = self.expect_identifier()?;
        let description = self.maybe_string();
        let technology = self.maybe_string();

        let mut rel = RelationshipNode {
            source,
            destination,
            description,
            technology,
            tags: Vec::new(),
            properties: HashMap::new(),
            perspectives: HashMap::new(),
        };

        // Check for optional block
        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();
            loop {
                self.skip_newlines_and_comments();

                match self.current_kind() {
                    Some(TokenKind::CloseBrace) => {
                        self.advance();
                        break;
                    }
                    Some(TokenKind::Tags) => {
                        self.advance();
                        let tags = self.expect_string()?;
                        rel.tags = tags.split(',').map(|s| s.trim().to_string()).collect();
                    }
                    Some(TokenKind::Properties) => {
                        self.advance();
                        rel.properties.extend(self.parse_properties_block()?);
                    }
                    Some(TokenKind::Perspectives) => {
                        self.advance();
                        rel.perspectives.extend(self.parse_perspectives_block()?);
                    }
                    _ => break,
                }
            }
        }

        Ok(rel)
    }

    fn parse_group(&mut self) -> Result<GroupNode> {
        self.skip_newlines_and_comments();
        let name = self.expect_string()?;

        let mut group = GroupNode {
            name,
            elements: Vec::new(),
            groups: Vec::new(),
        };

        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Group) => {
                    self.advance();
                    let nested = self.parse_group()?;
                    group.groups.push(nested);
                }
                Some(TokenKind::Person) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::Person, None)?;
                    group.elements.push(element);
                }
                Some(TokenKind::SoftwareSystem) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::SoftwareSystem, None)?;
                    group.elements.push(element);
                }
                Some(TokenKind::Identifier(id)) => {
                    let id = id.clone();
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Equals) {
                        self.advance();
                        self.skip_newlines_and_comments();

                        match self.current_kind() {
                            Some(TokenKind::Person) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::Person, Some(id))?;
                                group.elements.push(element);
                            }
                            Some(TokenKind::SoftwareSystem) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::SoftwareSystem, Some(id))?;
                                group.elements.push(element);
                            }
                            _ => break,
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(group)
    }

    /// Parse an enterprise block - elements defined inside are marked as internal
    fn parse_enterprise(&mut self) -> Result<EnterpriseNode> {
        self.skip_newlines_and_comments();
        let name = self.expect_string()?;

        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut elements = Vec::new();
        let mut groups = Vec::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Person) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::Person, None)?;
                    elements.push(element);
                }
                Some(TokenKind::SoftwareSystem) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::SoftwareSystem, None)?;
                    elements.push(element);
                }
                Some(TokenKind::Group) => {
                    self.advance();
                    let group = self.parse_group()?;
                    groups.push(group);
                }
                Some(TokenKind::Identifier(id)) => {
                    let id = id.clone();
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Equals) {
                        self.advance();
                        self.skip_newlines_and_comments();

                        match self.current_kind() {
                            Some(TokenKind::Person) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::Person, Some(id))?;
                                elements.push(element);
                            }
                            Some(TokenKind::SoftwareSystem) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::SoftwareSystem, Some(id))?;
                                elements.push(element);
                            }
                            _ => break,
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(EnterpriseNode { name, elements, groups })
    }

    fn parse_deployment_environment(&mut self) -> Result<DeploymentEnvironmentNode> {
        self.skip_newlines_and_comments();
        let name = self.expect_string()?;

        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut children = Vec::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::DeploymentNode) => {
                    self.advance();
                    let element = self.parse_element(ElementKind::DeploymentNode, None)?;
                    children.push(element);
                }
                Some(TokenKind::DeploymentGroup) => {
                    return Err(Error::Parse(ParseError {
                        message: "Deployment groups are not yet supported by the native preview".into(),
                        location: self.current().map(|token| token.location),
                    }));
                }
                Some(TokenKind::Identifier(id)) => {
                    let id = id.clone();
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Equals) {
                        self.advance();
                        self.skip_newlines_and_comments();

                        match self.current_kind() {
                            Some(TokenKind::DeploymentNode) => {
                                self.advance();
                                let element = self.parse_element(ElementKind::DeploymentNode, Some(id))?;
                                children.push(element);
                            }
                            Some(TokenKind::DeploymentGroup) => {
                                return Err(Error::Parse(ParseError {
                                    message: "Deployment groups are not yet supported by the native preview".into(),
                                    location: self.current().map(|token| token.location),
                                }));
                            }
                            _ => {
                                return Err(Error::Parse(ParseError {
                                    message: "Expected deploymentNode or deploymentGroup after '='".to_string(),
                                    location: self.current().map(|t| t.location),
                                }));
                            }
                        }
                    } else {
                        return Err(Error::Parse(ParseError {
                            message: "Expected '=' after identifier in deploymentEnvironment".to_string(),
                            location: self.current().map(|t| t.location),
                        }));
                    }
                }
                Some(kind) => {
                    return Err(Error::Parse(ParseError {
                        message: format!("Unexpected token in deploymentEnvironment: {:?}", kind),
                        location: self.current().map(|t| t.location),
                    }));
                }
                None => {
                    return Err(Error::Parse(ParseError {
                        message: "Unexpected end of input in deploymentEnvironment".to_string(),
                        location: None,
                    }));
                }
            }
        }

        Ok(DeploymentEnvironmentNode { name, children })
    }

    fn parse_views(&mut self) -> Result<ViewsNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut views = ViewsNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::SystemLandscape) => {
                    self.advance();
                    let view = self.parse_system_landscape_view()?;
                    views.system_landscape.push(view);
                }
                Some(TokenKind::SystemContext) => {
                    self.advance();
                    let view = self.parse_system_context_view()?;
                    views.system_context.push(view);
                }
                Some(TokenKind::Container) => {
                    self.advance();
                    let view = self.parse_container_view()?;
                    views.container.push(view);
                }
                Some(TokenKind::Component) => {
                    self.advance();
                    let view = self.parse_component_view()?;
                    views.component.push(view);
                }
                Some(TokenKind::Dynamic) => {
                    self.advance();
                    let view = self.parse_dynamic_view()?;
                    views.dynamic.push(view);
                }
                Some(TokenKind::Deployment) => {
                    self.advance();
                    let view = self.parse_deployment_view()?;
                    views.deployment.push(view);
                }
                Some(TokenKind::Filtered) => {
                    self.advance();
                    let view = self.parse_filtered_view()?;
                    views.filtered.push(view);
                }
                Some(TokenKind::Custom) => {
                    self.advance();
                    let view = self.parse_custom_view()?;
                    views.custom.push(view);
                }
                Some(TokenKind::Image) => {
                    self.advance();
                    let view = self.parse_image_view()?;
                    views.image.push(view);
                }
                Some(TokenKind::Styles) => {
                    self.advance();
                    views.styles = Some(self.parse_styles()?);
                }
                Some(TokenKind::Theme) => {
                    self.advance();
                    let url = self.expect_string()?;
                    views.themes.push(url);
                }
                Some(TokenKind::Themes) => {
                    self.advance();
                    while let Some(url) = self.maybe_string() {
                        views.themes.push(url);
                    }
                }
                Some(TokenKind::Branding) => {
                    self.advance();
                    views.branding = Some(self.parse_branding()?);
                }
                Some(TokenKind::Terminology) => {
                    self.advance();
                    views.terminology = Some(self.parse_terminology()?);
                }
                Some(TokenKind::Properties) => {
                    // Skip properties block in views
                    self.advance();
                    self.skip_block()?;
                }
                Some(kind) => {
                    return Err(Error::Parse(ParseError {
                        message: format!("Unexpected token in views: {:?}", kind),
                        location: self.current().map(|t| t.location),
                    }));
                }
                None => break,
            }
        }

        Ok(views)
    }

    /// Consume one common view annotation, leaving interaction/selection grammar to its owner.
    fn parse_common_view_property(&mut self, properties: &mut ViewPropertiesNode) -> Result<bool> {
        match self.current_kind().cloned() {
            Some(TokenKind::AutoLayout) => {
                self.advance();
                properties.auto_layout = Some(self.parse_auto_layout()?);
            }
            Some(TokenKind::Properties) => {
                self.advance();
                properties.properties.extend(self.parse_properties_block()?);
            }
            Some(TokenKind::Title) => {
                self.advance();
                properties.title = Some(self.expect_string()?);
            }
            Some(TokenKind::Description) => {
                self.advance();
                properties.description = Some(self.expect_string()?);
            }
            Some(TokenKind::Default) => {
                self.advance();
                properties.default = true;
            }
            Some(TokenKind::Identifier(name))
                if name.eq_ignore_ascii_case("background")
                    && !self
                        .tokens
                        .get(self.position + 1)
                        .is_some_and(|token| token.kind == TokenKind::Arrow) =>
            {
                self.advance();
                properties.background = Some(self.expect_string()?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Read one selection statement without consuming the next physical line.
    fn parse_selection_operation(&mut self) -> Result<SelectionOperation> {
        let adding = self.check(&TokenKind::Include);
        let location = self.current().map(|token| token.location);
        self.advance();
        let mut entries = Vec::new();
        loop {
            let entry = match self.current_kind().cloned() {
                Some(TokenKind::Star) => IncludeExclude::All,
                Some(TokenKind::Identifier(name)) => IncludeExclude::Identifier(name),
                Some(TokenKind::String(expression)) => IncludeExclude::Expression(expression),
                _ => break,
            };
            entries.push(entry);
            self.advance();
        }
        if entries.is_empty() {
            return Err(Error::Parse(ParseError {
                message: "Selection requires at least one identifier, wildcard or expression"
                    .into(),
                location,
            }));
        }
        Ok(if adding {
            SelectionOperation::Include(entries)
        } else {
            SelectionOperation::Exclude(entries)
        })
    }

    fn parse_view_properties(&mut self) -> Result<ViewPropertiesNode> {
        let key = self.maybe_string();
        let title = self.maybe_string();
        let description = self.maybe_string();

        let mut props = ViewPropertiesNode {
            key,
            title,
            description,
            selection: Vec::new(),
            auto_layout: None,
            properties: HashMap::new(),
            background: None,
            animations: Vec::new(),
            default: false,
        };

        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();

            loop {
                self.skip_newlines_and_comments();
                if self.parse_common_view_property(&mut props)? { continue; }

                match self.current_kind().cloned() {
                    Some(TokenKind::CloseBrace) => {
                        self.advance();
                        break;
                    }
                    Some(TokenKind::Include | TokenKind::Exclude) => {
                        props.selection.push(self.parse_selection_operation()?);
                    }
                    Some(TokenKind::Animation) => {
                        self.advance();
                        let step = self.parse_animation_step()?;
                        props.animations.push(step);
                    }
                    Some(TokenKind::Identifier(ref id)) => {
                        match id.to_lowercase().as_str() {
                            "background" => {
                                self.advance();
                                props.background = Some(self.expect_string()?);
                            }
                            _ => break,
                        }
                    }
                    _ => break,
                }
            }
        }

        Ok(props)
    }

    /// Parse an animation step: animation { element1, element2, ... }
    fn parse_animation_step(&mut self) -> Result<AnimationStepNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut elements = Vec::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Identifier(id)) => {
                    self.advance();
                    elements.push(id);
                    // Skip optional comma
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                    }
                }
                _ => break,
            }
        }

        Ok(AnimationStepNode { elements })
    }

    fn parse_auto_layout(&mut self) -> Result<AutoLayoutNode> {
        self.skip_newlines_and_comments();

        let direction = if let Some(TokenKind::Identifier(id)) = self.current_kind().cloned() {
            // Only consume the identifier if it's a known direction
            match id.to_lowercase().as_str() {
                "tb" => {
                    self.advance();
                    crate::ast::AutoLayoutDirection::TopBottom
                }
                "bt" => {
                    self.advance();
                    crate::ast::AutoLayoutDirection::BottomTop
                }
                "lr" => {
                    self.advance();
                    crate::ast::AutoLayoutDirection::LeftRight
                }
                "rl" => {
                    self.advance();
                    crate::ast::AutoLayoutDirection::RightLeft
                }
                _ => crate::ast::AutoLayoutDirection::TopBottom,
            }
        } else {
            crate::ast::AutoLayoutDirection::TopBottom
        };

        let rank_separation = if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
            self.advance();
            Some(n as u32)
        } else {
            None
        };

        let node_separation = if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
            self.advance();
            Some(n as u32)
        } else {
            None
        };

        Ok(AutoLayoutNode {
            direction,
            rank_separation,
            node_separation,
        })
    }

    fn parse_system_landscape_view(&mut self) -> Result<SystemLandscapeViewNode> {
        let properties = self.parse_view_properties()?;
        Ok(SystemLandscapeViewNode { properties })
    }

    fn parse_system_context_view(&mut self) -> Result<SystemContextViewNode> {
        self.skip_newlines_and_comments();
        let software_system = self.expect_identifier()?;
        let properties = self.parse_view_properties()?;
        Ok(SystemContextViewNode {
            software_system,
            properties,
        })
    }

    fn parse_container_view(&mut self) -> Result<ContainerViewNode> {
        self.skip_newlines_and_comments();
        let software_system = self.expect_identifier()?;
        let properties = self.parse_view_properties()?;
        Ok(ContainerViewNode {
            software_system,
            properties,
        })
    }

    fn parse_component_view(&mut self) -> Result<ComponentViewNode> {
        self.skip_newlines_and_comments();
        let container = self.expect_identifier()?;
        let properties = self.parse_view_properties()?;
        Ok(ComponentViewNode {
            container,
            properties,
        })
    }

    fn parse_dynamic_view(&mut self) -> Result<DynamicViewNode> {
        self.skip_newlines_and_comments();

        // Parse optional scope (element identifier or *)
        let scope = if let Some(TokenKind::Identifier(_)) = self.current_kind() {
            Some(self.expect_identifier()?)
        } else if self.check(&TokenKind::Star) {
            self.advance();
            None
        } else {
            None
        };

        // Parse optional key, title, description strings
        let key = self.maybe_string();
        let title = self.maybe_string();
        let description = self.maybe_string();

        let mut props = ViewPropertiesNode {
            key,
            title,
            description,
            selection: Vec::new(),
            auto_layout: None,
            properties: HashMap::new(),
            background: None,
            animations: Vec::new(),
            default: false,
        };

        let mut content = Vec::new();

        // Parse the block if present
        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();
            self.parse_dynamic_view_body(&mut props, &mut content)?;
        }

        Ok(DynamicViewNode {
            scope,
            properties: props,
            content,
        })
    }

    /// Parse the body of a dynamic view, handling view properties, steps, and parallel sequences.
    fn parse_dynamic_view_body(
        &mut self,
        props: &mut ViewPropertiesNode,
        content: &mut Vec<DynamicContent>,
    ) -> Result<()> {
        loop {
            self.skip_newlines_and_comments();
            if self.parse_common_view_property(props)? {
                continue;
            }

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::OpenBrace) => {
                    // Start of a parallel sequence block
                    self.advance();
                    let parallel = self.parse_parallel_sequence()?;
                    content.push(DynamicContent::ParallelSequence(parallel));
                }
                Some(TokenKind::Include | TokenKind::Exclude) => {
                    props.selection.push(self.parse_selection_operation()?);
                }
                Some(TokenKind::Number(_)) => {
                    content.push(DynamicContent::Step(self.parse_ordered_dynamic_step()?));
                }
                Some(TokenKind::Identifier(source)) => {
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Arrow) {
                        // This is a dynamic step: source -> destination "description"
                        self.advance();
                        let step = self.parse_dynamic_step(source)?;
                        content.push(DynamicContent::Step(step));
                    } else {
                        return Err(Error::Parse(ParseError {
                            message: "Expected -> after dynamic source (relationship references are not supported)".into(),
                            location: self.current().map(|token| token.location),
                        }));
                    }
                }
                _ => break,
            }
        }

        Ok(())
    }

    /// Parse a parallel sequence block: { content... }
    /// Called after the opening brace has been consumed.
    fn parse_parallel_sequence(&mut self) -> Result<ParallelSequenceNode> {
        let mut content = Vec::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::OpenBrace) => {
                    // Nested parallel sequence
                    self.advance();
                    let nested = self.parse_parallel_sequence()?;
                    content.push(DynamicContent::ParallelSequence(nested));
                }
                Some(TokenKind::Number(_)) => {
                    content.push(DynamicContent::Step(self.parse_ordered_dynamic_step()?));
                }
                Some(TokenKind::Identifier(source)) => {
                    self.advance();
                    self.skip_newlines_and_comments();

                    if self.check(&TokenKind::Arrow) {
                        self.advance();
                        let step = self.parse_dynamic_step(source)?;
                        content.push(DynamicContent::Step(step));
                    } else {
                        return Err(Error::Parse(ParseError {
                            message: "Expected -> after parallel interaction source".into(),
                            location: self.current().map(|token| token.location),
                        }));
                    }
                }
                _ => break,
            }
        }

        Ok(ParallelSequenceNode { content })
    }

    /// Parse a single dynamic step: destination "description"
    /// (source was already consumed and is passed in)
    fn parse_ordered_dynamic_step(&mut self) -> Result<DynamicStepNode> {
        let mut order = String::new();
        loop {
            match self.current_kind().cloned() {
                Some(TokenKind::Number(value)) => {
                    order.push_str(&value.to_string());
                    self.advance();
                }
                _ => {
                    return Err(Error::Parse(ParseError {
                        message: "Expected numeric dynamic order".into(),
                        location: self.current().map(|token| token.location),
                    }))
                }
            }
            if !self.check(&TokenKind::Dot) {
                break;
            }
            self.advance();
            order.push('.');
        }
        self.expect(TokenKind::Colon)?;
        let source = self.expect_identifier()?;
        self.expect(TokenKind::Arrow)?;
        let mut step = self.parse_dynamic_step(source)?;
        step.order = Some(order);
        Ok(step)
    }

    fn parse_dynamic_step(&mut self, source: String) -> Result<DynamicStepNode> {
        self.skip_newlines_and_comments();
        let destination = self.expect_identifier()?;
        let description = self.maybe_string();
        let technology = self.maybe_string();

        Ok(DynamicStepNode {
            source,
            destination,
            description,
            technology,
            order: None,
        })
    }

    fn parse_deployment_view(&mut self) -> Result<DeploymentViewNode> {
        self.skip_newlines_and_comments();
        let scope = if let Some(TokenKind::Identifier(_)) = self.current_kind() {
            Some(self.expect_identifier()?)
        } else if self.check(&TokenKind::Star) {
            self.advance();
            None
        } else {
            None
        };

        let environment = self.expect_string()?;
        let properties = self.parse_view_properties()?;

        Ok(DeploymentViewNode {
            scope,
            environment,
            properties,
        })
    }

    /// Parse a filtered view: filtered <baseViewKey> <include|exclude> "tag1,tag2" [key] [description] { ... }
    fn parse_filtered_view(&mut self) -> Result<FilteredViewNode> {
        self.skip_newlines_and_comments();

        // Base view key
        let base_view = self.expect_identifier_or_string()?;

        // Mode: include or exclude
        self.skip_newlines_and_comments();
        let mode = if let Some(TokenKind::Include) = self.current_kind() {
            self.advance();
            crate::ast::FilterMode::Include
        } else if let Some(TokenKind::Exclude) = self.current_kind() {
            self.advance();
            crate::ast::FilterMode::Exclude
        } else {
            crate::ast::FilterMode::Include // Default
        };

        // Tags (comma-separated string)
        let tags_str = self.maybe_string().unwrap_or_default();
        let tags: Vec<String> = tags_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Optional key and description
        let properties = self.parse_view_properties()?;

        Ok(FilteredViewNode {
            base_view,
            mode,
            tags,
            properties,
        })
    }

    /// Parse a custom view: custom [key] [title] [description] { ... }
    fn parse_custom_view(&mut self) -> Result<CustomViewNode> {
        let properties = self.parse_view_properties()?;
        Ok(CustomViewNode { properties })
    }

    /// Parse an image view: image <element|*> [key] [description] { <type> <url> title "title" }
    fn parse_image_view(&mut self) -> Result<ImageViewNode> {
        self.skip_newlines_and_comments();

        // Element identifier or * for workspace-level
        let element = if self.check(&TokenKind::Star) {
            self.advance();
            None
        } else if let Some(TokenKind::Identifier(id)) = self.current_kind() {
            let id = id.clone();
            self.advance();
            Some(id)
        } else {
            self.maybe_string()
        };

        // Parse key and description (strings before the block)
        let key = self.maybe_string();
        let description = self.maybe_string();

        let mut properties = ViewPropertiesNode::default();
        properties.key = key;
        properties.description = description;

        // Parse the image view body for content type and URL
        let mut content_type: Option<String> = None;
        let mut content: Option<String> = None;

        // Check if there's a block to parse
        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();

            loop {
                self.skip_newlines_and_comments();

                match self.current_kind().cloned() {
                    Some(TokenKind::CloseBrace) => {
                        self.advance();
                        break;
                    }
                    Some(TokenKind::Title) => {
                        self.advance();
                        properties.title = self.maybe_string();
                    }
                    Some(TokenKind::Properties) => {
                        self.advance();
                        properties.properties.extend(self.parse_properties_block()?);
                    }
                    Some(TokenKind::Identifier(id)) => {
                        // This could be the content type (plantuml, mermaid, image)
                        content_type = Some(id.clone());
                        self.advance();
                        // Next should be the URL or content
                        content = self.maybe_string();
                    }
                    _ => {
                        // Skip unknown tokens
                        self.advance();
                    }
                }
            }
        }

        Ok(ImageViewNode {
            element,
            properties,
            content_type,
            content,
        })
    }

    fn parse_styles(&mut self) -> Result<StylesNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut styles = StylesNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Element) => {
                    self.advance();
                    let style = self.parse_element_style()?;
                    styles.elements.push(style);
                }
                Some(TokenKind::Relationship) => {
                    self.advance();
                    let style = self.parse_relationship_style()?;
                    styles.relationships.push(style);
                }
                _ => break,
            }
        }

        Ok(styles)
    }

    fn parse_element_style(&mut self) -> Result<ElementStyleNode> {
        self.skip_newlines_and_comments();
        let tag = self.expect_string()?;

        let mut style = ElementStyleNode {
            tag,
            shape: None,
            icon: None,
            icon_position: None,
            width: None,
            height: None,
            background: None,
            color: None,
            stroke: None,
            stroke_width: None,
            font_size: None,
            border: None,
            opacity: None,
            metadata: None,
            description: None,
        };

        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();

            loop {
                self.skip_newlines_and_comments();

                match self.current_kind().cloned() {
                    Some(TokenKind::CloseBrace) => {
                        self.advance();
                        break;
                    }
                    Some(TokenKind::Identifier(prop)) => {
                        self.advance();
                        match prop.to_lowercase().as_str() {
                            "shape" => {
                                // Shape can be Person (which is a keyword) or other identifiers
                                style.shape = Some(self.expect_identifier_or_shape_keyword()?);
                            }
                            "icon" => style.icon = Some(self.expect_string()?),
                            "iconposition" => {
                                style.icon_position = Some(self.expect_identifier()?);
                            }
                            "width" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.width = Some(n as u32);
                                }
                            }
                            "height" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.height = Some(n as u32);
                                }
                            }
                            "background" => style.background = Some(self.expect_string()?),
                            "color" | "colour" => style.color = Some(self.expect_string()?),
                            "stroke" => style.stroke = Some(self.expect_string()?),
                            "strokewidth" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.stroke_width = Some(n as u32);
                                }
                            }
                            "fontsize" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.font_size = Some(n as u32);
                                }
                            }
                            "border" => style.border = Some(self.expect_identifier()?),
                            "opacity" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.opacity = Some(n as u32);
                                }
                            }
                            "metadata" => {
                                let val = self.expect_identifier()?;
                                style.metadata = Some(val.to_lowercase() == "true");
                            }
                            "description" => {
                                let val = self.expect_identifier()?;
                                style.description = Some(val.to_lowercase() == "true");
                            }
                            _ => {}
                        }
                    }
                    _ => break,
                }
            }
        }

        Ok(style)
    }

    fn parse_relationship_style(&mut self) -> Result<RelationshipStyleNode> {
        self.skip_newlines_and_comments();
        let tag = self.expect_string()?;

        let mut style = RelationshipStyleNode {
            tag,
            thickness: None,
            color: None,
            style: None,
            routing: None,
            font_size: None,
            width: None,
            position: None,
            opacity: None,
        };

        self.skip_newlines_and_comments();
        if self.check(&TokenKind::OpenBrace) {
            self.advance();

            loop {
                self.skip_newlines_and_comments();

                match self.current_kind().cloned() {
                    Some(TokenKind::CloseBrace) => {
                        self.advance();
                        break;
                    }
                    Some(TokenKind::Identifier(prop)) => {
                        self.advance();
                        match prop.to_lowercase().as_str() {
                            "thickness" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.thickness = Some(n as u32);
                                }
                            }
                            "color" | "colour" => style.color = Some(self.expect_string()?),
                            "style" => style.style = Some(self.expect_identifier()?),
                            "routing" => style.routing = Some(self.expect_identifier()?),
                            "fontsize" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.font_size = Some(n as u32);
                                }
                            }
                            "width" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.width = Some(n as u32);
                                }
                            }
                            "position" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.position = Some(n as u32);
                                }
                            }
                            "opacity" => {
                                if let Some(TokenKind::Number(n)) = self.current_kind().cloned() {
                                    self.advance();
                                    style.opacity = Some(n as u32);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => break,
                }
            }
        }

        Ok(style)
    }

    fn parse_properties_block(&mut self) -> Result<HashMap<String, String>> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut props = HashMap::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::String(_)) => {
                    let key = self.expect_string()?;
                    let value = self.expect_string()?;
                    props.insert(key, value);
                }
                _ => break,
            }
        }

        Ok(props)
    }

    /// Parse a perspectives block: perspectives { "Name" "Description" ... }
    fn parse_perspectives_block(&mut self) -> Result<HashMap<String, String>> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut perspectives = HashMap::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::String(_)) => {
                    let name = self.expect_string()?;
                    let description = self.expect_string()?;
                    perspectives.insert(name, description);
                }
                _ => break,
            }
        }

        Ok(perspectives)
    }

    /// Parse a configuration block: configuration { scope ... visibility ... users { ... } }
    fn parse_configuration(&mut self) -> Result<ConfigurationNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut config = ConfigurationNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Identifier(ref id)) if id.to_lowercase() == "scope" => {
                    self.advance();
                    let scope = self.expect_identifier_or_keyword()?;
                    config.scope = Some(scope);
                }
                Some(TokenKind::Identifier(ref id)) if id.to_lowercase() == "visibility" => {
                    self.advance();
                    let vis = self.expect_identifier_or_keyword()?;
                    config.visibility = Some(vis);
                }
                Some(TokenKind::Identifier(ref id)) if id.to_lowercase() == "users" => {
                    self.advance();
                    config.users = self.parse_users_block()?;
                }
                Some(TokenKind::Properties) => {
                    // Skip properties block in configuration
                    self.advance();
                    self.skip_block()?;
                }
                _ => break,
            }
        }

        Ok(config)
    }

    /// Parse a users block: users { username readwrite/readonly ... }
    fn parse_users_block(&mut self) -> Result<Vec<UserNode>> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut users = Vec::new();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Identifier(_)) | Some(TokenKind::String(_)) => {
                    let username = self.expect_identifier_or_string()?;
                    let role = self.expect_identifier()?;
                    users.push(UserNode { username, role });
                }
                _ => break,
            }
        }

        Ok(users)
    }

    /// Parse a script directive: !script <lang> { ... } or !script "path/to/file"
    fn parse_script_directive(&mut self) -> Result<ScriptDirective> {
        self.skip_newlines_and_comments();

        // Check if it's an inline script with language identifier or a file path
        match self.current_kind().cloned() {
            Some(TokenKind::String(path)) => {
                // External script file: !script "path/to/script.groovy"
                self.advance();
                Ok(ScriptDirective {
                    language: None,
                    content: None,
                    file_path: Some(path),
                })
            }
            Some(TokenKind::Identifier(lang)) => {
                // Inline script with language: !script groovy { ... }
                self.advance();
                self.skip_newlines_and_comments();

                // Expect a block
                if !self.check(&TokenKind::OpenBrace) {
                    return Err(Error::Parse(ParseError {
                        message: format!("Expected '{{' after script language '{}'", lang),
                        location: self.current().map(|t| t.location),
                    }));
                }

                let content = self.parse_script_block()?;

                Ok(ScriptDirective {
                    language: Some(lang),
                    content: Some(content),
                    file_path: None,
                })
            }
            Some(TokenKind::OpenBrace) => {
                // Inline script without explicit language (default to Lua)
                let content = self.parse_script_block()?;

                Ok(ScriptDirective {
                    language: Some("lua".to_string()),
                    content: Some(content),
                    file_path: None,
                })
            }
            other => Err(Error::Parse(ParseError {
                message: format!("Expected script language or file path, got {:?}", other),
                location: self.current().map(|t| t.location),
            })),
        }
    }

    /// Parse a script block and extract raw content between braces.
    fn parse_script_block(&mut self) -> Result<String> {
        self.expect(TokenKind::OpenBrace)?;

        // Track position after opening brace
        let start_pos = self.position;
        let mut brace_depth = 1;

        // Find the matching close brace
        while let Some(kind) = self.current_kind().cloned() {
            match kind {
                TokenKind::OpenBrace => {
                    brace_depth += 1;
                    self.advance();
                }
                TokenKind::CloseBrace => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        break;
                    }
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }

        let end_pos = self.position;
        self.advance(); // consume closing brace

        // Reconstruct the script content from tokens
        let mut content = String::new();
        let mut prev_line = 0;
        let mut prev_col = 0;

        for i in start_pos..end_pos {
            if let Some(token) = self.tokens.get(i) {
                // Add newlines/spaces based on line/column differences
                if prev_line > 0 {
                    if token.location.line > prev_line {
                        for _ in 0..(token.location.line - prev_line) {
                            content.push('\n');
                        }
                        prev_col = 0;
                    }
                    if token.location.column > prev_col {
                        for _ in 0..(token.location.column - prev_col - 1) {
                            content.push(' ');
                        }
                    }
                }

                // Add the token text
                let text = self.token_to_text(&token.kind);
                content.push_str(&text);
                prev_line = token.location.line;
                prev_col = token.location.column + token.len;
            }
        }

        Ok(content.trim().to_string())
    }

    /// Convert a token kind back to its text representation.
    fn token_to_text(&self, kind: &TokenKind) -> String {
        match kind {
            TokenKind::String(s) => format!("\"{}\"", s),
            TokenKind::Number(n) => n.to_string(),
            TokenKind::Identifier(id) => id.clone(),
            TokenKind::OpenBrace => "{".to_string(),
            TokenKind::CloseBrace => "}".to_string(),
            TokenKind::Arrow => "->".to_string(),
            TokenKind::Equals => "=".to_string(),
            TokenKind::Colon => ":".to_string(),
            TokenKind::Dot => ".".to_string(),
            TokenKind::Comma => ",".to_string(),
            TokenKind::OpenParen => "(".to_string(),
            TokenKind::CloseParen => ")".to_string(),
            TokenKind::Newline => "\n".to_string(),
            TokenKind::Comment => "".to_string(), // Skip comments
            TokenKind::Star => "*".to_string(),
            // DSL keywords - convert back to text for scripts
            TokenKind::Workspace => "workspace".to_string(),
            TokenKind::Model => "model".to_string(),
            TokenKind::Views => "views".to_string(),
            TokenKind::Person => "person".to_string(),
            TokenKind::SoftwareSystem => "softwareSystem".to_string(),
            TokenKind::Container => "container".to_string(),
            TokenKind::Component => "component".to_string(),
            TokenKind::Group => "group".to_string(),
            TokenKind::DeploymentNode => "deploymentNode".to_string(),
            TokenKind::DeploymentEnvironment => "deploymentEnvironment".to_string(),
            TokenKind::InfrastructureNode => "infrastructureNode".to_string(),
            TokenKind::ContainerInstance => "containerInstance".to_string(),
            TokenKind::SoftwareSystemInstance => "softwareSystemInstance".to_string(),
            TokenKind::SystemLandscape => "systemLandscape".to_string(),
            TokenKind::SystemContext => "systemContext".to_string(),
            TokenKind::Dynamic => "dynamic".to_string(),
            TokenKind::Deployment => "deployment".to_string(),
            TokenKind::Filtered => "filtered".to_string(),
            TokenKind::Custom => "custom".to_string(),
            TokenKind::Styles => "styles".to_string(),
            TokenKind::Element => "element".to_string(),
            TokenKind::Relationship => "relationship".to_string(),
            TokenKind::Theme => "theme".to_string(),
            TokenKind::Themes => "themes".to_string(),
            TokenKind::Include => "include".to_string(),
            TokenKind::Exclude => "exclude".to_string(),
            TokenKind::AutoLayout => "autoLayout".to_string(),
            TokenKind::Animation => "animation".to_string(),
            TokenKind::Properties => "properties".to_string(),
            TokenKind::Perspectives => "perspectives".to_string(),
            TokenKind::Tags => "tags".to_string(),
            TokenKind::Technology => "technology".to_string(),
            TokenKind::Description => "description".to_string(),
            TokenKind::Url => "url".to_string(),
            TokenKind::Extends => "extends".to_string(),
            TokenKind::Instances => "instances".to_string(),
            TokenKind::Enterprise => "enterprise".to_string(),
            TokenKind::CustomElement => "customElement".to_string(),
            TokenKind::Configuration => "configuration".to_string(),
            TokenKind::Branding => "branding".to_string(),
            TokenKind::Terminology => "terminology".to_string(),
            TokenKind::Default => "default".to_string(),
            TokenKind::Image => "image".to_string(),
            TokenKind::HealthCheck => "healthCheck".to_string(),
            // Directive keywords (as text in scripts)
            TokenKind::DirectiveConst => "const".to_string(),
            TokenKind::DirectiveIdentifiers => "identifiers".to_string(),
            TokenKind::DirectiveImpliedRelationships => "impliedRelationships".to_string(),
            TokenKind::DirectiveInclude => "include".to_string(),
            TokenKind::DirectiveDocs => "docs".to_string(),
            TokenKind::DirectiveAdrs => "adrs".to_string(),
            TokenKind::DirectiveRef => "ref".to_string(),
            TokenKind::DirectiveExtend => "extend".to_string(),
            TokenKind::DirectivePlugin => "plugin".to_string(),
            TokenKind::DirectiveScript => "script".to_string(),
            // Fallback for any other tokens
            _ => String::new(),
        }
    }

    /// Parse a branding block: branding { logo ... font ... }
    fn parse_branding(&mut self) -> Result<BrandingNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut branding = BrandingNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Logo) => {
                    self.advance();
                    branding.logo = Some(self.expect_string()?);
                }
                Some(TokenKind::Font) => {
                    self.advance();
                    let name = self.expect_string()?;
                    let url = self.maybe_string();
                    branding.font = Some(FontNode { name, url });
                }
                _ => break,
            }
        }

        Ok(branding)
    }

    /// Parse a terminology block: terminology { person "Custom" ... }
    fn parse_terminology(&mut self) -> Result<TerminologyNode> {
        self.skip_newlines_and_comments();
        self.expect(TokenKind::OpenBrace)?;

        let mut terminology = TerminologyNode::default();

        loop {
            self.skip_newlines_and_comments();

            match self.current_kind().cloned() {
                Some(TokenKind::CloseBrace) => {
                    self.advance();
                    break;
                }
                Some(TokenKind::Enterprise) => {
                    self.advance();
                    terminology.enterprise = Some(self.expect_string()?);
                }
                Some(TokenKind::Person) => {
                    self.advance();
                    terminology.person = Some(self.expect_string()?);
                }
                Some(TokenKind::SoftwareSystem) => {
                    self.advance();
                    terminology.software_system = Some(self.expect_string()?);
                }
                Some(TokenKind::Container) => {
                    self.advance();
                    terminology.container = Some(self.expect_string()?);
                }
                Some(TokenKind::Component) => {
                    self.advance();
                    terminology.component = Some(self.expect_string()?);
                }
                Some(TokenKind::DeploymentNode) => {
                    self.advance();
                    terminology.deployment_node = Some(self.expect_string()?);
                }
                Some(TokenKind::InfrastructureNode) => {
                    self.advance();
                    terminology.infrastructure_node = Some(self.expect_string()?);
                }
                Some(TokenKind::Relationship) => {
                    self.advance();
                    terminology.relationship = Some(self.expect_string()?);
                }
                _ => break,
            }
        }

        Ok(terminology)
    }
}

/// Substitute constants in a string using the provided constants map.
fn substitute_constants(s: &str, constants: &HashMap<String, String>) -> String {
    let mut result = s.to_string();
    for (name, value) in constants {
        let pattern = format!("${{{}}}", name);
        result = result.replace(&pattern, value);
    }
    result
}

/// Apply constant substitution to all strings in the AST.
fn apply_constants_to_ast(mut ast: WorkspaceNode, constants: &HashMap<String, String>) -> WorkspaceNode {
    // Substitute in workspace properties
    if let Some(name) = ast.name {
        ast.name = Some(substitute_constants(&name, constants));
    }
    if let Some(desc) = ast.description {
        ast.description = Some(substitute_constants(&desc, constants));
    }

    // Substitute in model
    if let Some(mut model) = ast.model {
        model = apply_constants_to_model(model, constants);
        ast.model = Some(model);
    }

    // Substitute in views
    if let Some(mut views) = ast.views {
        views = apply_constants_to_views(views, constants);
        ast.views = Some(views);
    }

    ast
}

/// Apply constant substitution to model elements.
fn apply_constants_to_model(mut model: ModelNode, constants: &HashMap<String, String>) -> ModelNode {
    for element in &mut model.elements {
        apply_constants_to_element(element, constants);
    }
    for rel in &mut model.relationships {
        apply_constants_to_relationship(rel, constants);
    }
    for group in &mut model.groups {
        apply_constants_to_group(group, constants);
    }
    model
}

/// Apply constant substitution to a single element.
fn apply_constants_to_element(element: &mut ElementNode, constants: &HashMap<String, String>) {
    element.name = substitute_constants(&element.name, constants);
    if let Some(desc) = &element.description {
        element.description = Some(substitute_constants(desc, constants));
    }
    if let Some(tech) = &element.technology {
        element.technology = Some(substitute_constants(tech, constants));
    }
    for child in &mut element.children {
        apply_constants_to_element(child, constants);
    }
    for rel in &mut element.relationships {
        apply_constants_to_relationship(rel, constants);
    }
}

/// Apply constant substitution to a relationship.
fn apply_constants_to_relationship(rel: &mut RelationshipNode, constants: &HashMap<String, String>) {
    if let Some(desc) = &rel.description {
        rel.description = Some(substitute_constants(desc, constants));
    }
    if let Some(tech) = &rel.technology {
        rel.technology = Some(substitute_constants(tech, constants));
    }
}

/// Apply constant substitution to a group.
fn apply_constants_to_group(group: &mut GroupNode, constants: &HashMap<String, String>) {
    group.name = substitute_constants(&group.name, constants);
    for element in &mut group.elements {
        apply_constants_to_element(element, constants);
    }
    for nested_group in &mut group.groups {
        apply_constants_to_group(nested_group, constants);
    }
}

/// Apply constant substitution to views.
fn apply_constants_to_views(mut views: ViewsNode, constants: &HashMap<String, String>) -> ViewsNode {
    for view in &mut views.system_landscape {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.system_context {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.container {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.component {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.dynamic {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.custom {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    for view in &mut views.deployment {
        apply_constants_to_view_properties(&mut view.properties, constants);
    }
    views
}

/// Apply constant substitution to view properties.
fn apply_constants_to_view_properties(props: &mut ViewPropertiesNode, constants: &HashMap<String, String>) {
    if let Some(key) = &props.key {
        props.key = Some(substitute_constants(key, constants));
    }
    if let Some(title) = &props.title {
        props.title = Some(substitute_constants(title, constants));
    }
    if let Some(desc) = &props.description {
        props.description = Some(substitute_constants(desc, constants));
    }
}

/// Lift relationship endpoints through containment without inventing transitive edges.
fn generate_implied_relationships(workspace: &mut Workspace) {
    let model = workspace.model();
    let mut pairs: HashSet<_> = model
        .relationships
        .iter()
        .map(|relationship| (relationship.source_id, relationship.destination_id))
        .collect();
    let mut implied = Vec::new();
    for relationship in &model.relationships {
        let sources: Vec<_> = std::iter::once(relationship.source_id)
            .chain(model.ancestor_ids(relationship.source_id))
            .collect();
        let destinations: Vec<_> = std::iter::once(relationship.destination_id)
            .chain(model.ancestor_ids(relationship.destination_id))
            .collect();
        for &source in &sources {
            for &destination in &destinations {
                // Self/containment edges are not valid implied cross-element links.
                // Reserve all explicit pairs before deriving any ancestors: an
                // authored relationship wins regardless of statement order.
                if sources.contains(&destination)
                    || destinations.contains(&source)
                    || !pairs.insert((source, destination))
                {
                    continue;
                }
                let mut lifted = relationship.clone();
                lifted.id = ElementId::new();
                lifted.source_id = source;
                lifted.destination_id = destination;
                implied.push(lifted);
            }
        }
    }
    workspace.model_mut().relationships.extend(implied);
}

/// View type for element resolution.
#[derive(Debug, Clone, Copy)]
enum ViewType {
    SystemLandscape,
    SystemContext,
    Container,
    Component,
    Custom,
}

/// Whether an explicitly named element is valid at this view's abstraction level.
fn static_view_allows(
    model: &Model,
    kind: ViewType,
    scope: Option<ElementId>,
    element: ElementRef<'_>,
) -> bool {
    match kind {
        ViewType::Custom => matches!(element, ElementRef::CustomElement(_)),
        ViewType::SystemLandscape | ViewType::SystemContext => matches!(
            element,
            ElementRef::Person(_) | ElementRef::SoftwareSystem(_)
        ),
        ViewType::Container => match element {
            ElementRef::Person(_) | ElementRef::Container(_) => true,
            ElementRef::SoftwareSystem(system) => Some(system.id()) != scope,
            _ => false,
        },
        ViewType::Component => match element {
            ElementRef::Person(_) | ElementRef::Component(_) => true,
            ElementRef::Container(container) => Some(container.id()) != scope,
            ElementRef::SoftwareSystem(system) => {
                Some(system.id()) != scope.and_then(|id| model.ancestor_ids(id).last().copied())
            }
            _ => false,
        },
    }
}

/// Resolve wildcard seeds and their permitted direct neighbours, never transitive reachability.
fn default_static_elements(
    model: &Model,
    kind: ViewType,
    scope: Option<ElementId>,
) -> Result<HashSet<ElementId>> {
    let focused = scope.and_then(|id| model.find_element(id));
    let seeds: HashSet<_> = match (kind, focused) {
        (ViewType::Custom, _) => model.custom_elements.iter().map(|element| element.id()).collect(),
        (ViewType::SystemLandscape, _) => model
            .architecture_elements()
            .filter(|element| {
                matches!(
                    element,
                    ElementRef::Person(_) | ElementRef::SoftwareSystem(_)
                )
            })
            .map(|element| element.id())
            .collect(),
        (ViewType::SystemContext, Some(ElementRef::SoftwareSystem(system))) => {
            [system.id()].into_iter().collect()
        }
        (ViewType::Container, Some(ElementRef::SoftwareSystem(system))) => system
            .containers
            .iter()
            .map(|container| container.id())
            .collect(),
        (ViewType::Component, Some(ElementRef::Container(container))) => container
            .components
            .iter()
            .map(|component| component.id())
            .collect(),
        _ => {
            return Err(Error::Parse(ParseError {
                message: "Unknown or invalid scope for static view".into(),
                location: None,
            }))
        }
    };
    let parent = scope.and_then(|id| model.ancestor_ids(id).last().copied());
    let neighbours: HashSet<_> = model
        .architecture_elements()
        .filter(|element| {
            if !static_view_allows(model, kind, scope, *element) {
                return false;
            }
            match (kind, element) {
                (ViewType::Container, ElementRef::Container(_)) => false,
                (ViewType::Component, ElementRef::Component(_)) => false,
                (ViewType::Component, ElementRef::Container(container)) => {
                    model.ancestor_ids(container.id()).last().copied() == parent
                }
                _ => true,
            }
        })
        .map(|element| element.id())
        .collect();
    let mut selected = seeds.clone();
    selected.extend(
        model
            .relationships
            .iter()
            .flat_map(|relationship| {
                [
                    (relationship.source_id, relationship.destination_id),
                    (relationship.destination_id, relationship.source_id),
                ]
            })
            .filter(|(source, destination)| {
                seeds.contains(source) && neighbours.contains(destination)
            })
            .map(|(_, destination)| destination),
    );
    Ok(selected)
}

/// Resolve a named selection term to a permitted model element.
fn resolve_view_element(
    model: &Model,
    kind: ViewType,
    scope: Option<ElementId>,
    name: &str,
    identifiers: &HashMap<String, ElementId>,
) -> Result<ElementId> {
    let id = *identifiers.get(name).ok_or_else(|| {
        Error::Parse(ParseError {
            message: format!("Unknown element or unsupported view expression: '{name}'"),
            location: None,
        })
    })?;
    let element = model
        .find_element(id)
        .ok_or_else(|| Error::UndefinedIdentifier(name.into()))?;
    if !static_view_allows(model, kind, scope, element) {
        return Err(Error::Parse(ParseError {
            message: format!("Element '{name}' is not permitted in this view"),
            location: None,
        }));
    }
    Ok(id)
}

/// Resolve a complete element/relationship selection; an empty set stays empty.
fn resolve_static_selection(
    model: &Model,
    view_type: ViewType,
    scope_id: Option<ElementId>,
    operations: &[SelectionOperation],
    identifiers: &HashMap<String, ElementId>,
) -> Result<(Vec<ElementView>, Vec<RelationshipView>)> {
    let defaults = default_static_elements(model, view_type, scope_id)?;
    let mut selected = HashSet::new();
    for operation in operations {
        let (entries, adding) = match operation {
            SelectionOperation::Include(entries) => (entries, true),
            SelectionOperation::Exclude(entries) => (entries, false),
        };
        for entry in entries {
            match entry {
                IncludeExclude::All if adding => selected.extend(defaults.iter().copied()),
                IncludeExclude::All => selected.clear(),
                IncludeExclude::Identifier(name) | IncludeExclude::Expression(name) => {
                    let id = resolve_view_element(model, view_type, scope_id, name, identifiers)?;
                    if adding {
                        selected.insert(id);
                    } else {
                        selected.remove(&id);
                    }
                }
            }
        }
    }
    // A context view always contains its scoped system, including after exclude *.
    if matches!(view_type, ViewType::SystemContext) {
        selected.extend(scope_id);
    }
    for &id in &selected {
        if model
            .ancestor_ids(id)
            .iter()
            .any(|ancestor| selected.contains(ancestor))
        {
            return Err(Error::Parse(ParseError {
                message: "A view cannot contain both an element and its ancestor".into(),
                location: None,
            }));
        }
    }
    let elements = model
        .architecture_elements()
        .chain(model.custom_elements.iter().map(ElementRef::CustomElement))
        .filter(|element| selected.contains(&element.id()))
        .map(|element| ElementView::new(element.id()))
        .collect();
    let relationships = model
        .relationships
        .iter()
        .filter(|relationship| {
            selected.contains(&relationship.source_id)
                && selected.contains(&relationship.destination_id)
        })
        .map(|relationship| RelationshipView::new(relationship.id))
        .collect();
    Ok((elements, relationships))
}

/// Normalize common authored view annotations without evaluating selectors.
fn build_view_properties(
    source: ViewPropertiesNode,
    fallback_key: String,
) -> (ViewProperties, Vec<SelectionOperation>) {
    let mut properties = ViewProperties::new(source.key.unwrap_or(fallback_key));
    properties.title = source.title;
    properties.description = source.description;
    properties.background = source.background;
    properties.default = source.default;
    properties.auto_layout = source.auto_layout.map(convert_auto_layout);
    properties.properties = source.properties;
    (properties, source.selection)
}

/// Preserve common authored properties and resolve static selections once.
fn build_static_properties(
    model: &Model,
    kind: ViewType,
    scope: Option<ElementId>,
    source: ViewPropertiesNode,
    identifiers: &HashMap<String, ElementId>,
    fallback_key: String,
) -> Result<ViewProperties> {
    let (mut properties, selection) = build_view_properties(source, fallback_key);
    (properties.elements, properties.relationships) =
        resolve_static_selection(model, kind, scope, &selection, identifiers)?;
    Ok(properties)
}

/// Build a Workspace from an AST.
fn build_workspace(mut ast: WorkspaceNode) -> Result<Workspace> {
    // Step 1: Process !const directives to build constants map
    let mut constants = HashMap::new();
    for directive in &ast.directives {
        if let Directive::Const { name, value } = directive {
            constants.insert(name.clone(), value.clone());
        }
    }

    // Step 2: Apply constant substitution to all strings in the AST
    if !constants.is_empty() {
        ast = apply_constants_to_ast(ast, &constants);
    }

    // Step 3: Check for !impliedRelationships directive
    let mut implied_relationships_enabled = true;
    for directive in &ast.directives {
        if let Directive::ImpliedRelationships(enabled) = directive {
            implied_relationships_enabled = *enabled;
        }
    }

    // Step 4: Build the workspace normally
    let mut workspace = Workspace::new(
        ast.name.unwrap_or_else(|| "Untitled".to_string()),
        ast.description.unwrap_or_default(),
    );

    workspace.properties = ast.properties;

    // Step 5: Process workspace configuration block if present
    if let Some(config_node) = ast.configuration {
        let mut ws_config = WorkspaceConfiguration::default();

        // Parse scope
        if let Some(scope_str) = config_node.scope {
            ws_config.scope = match scope_str.to_lowercase().as_str() {
                "landscape" => Some(WorkspaceScope::Landscape),
                "softwaresystem" => Some(WorkspaceScope::SoftwareSystem),
                "none" => Some(WorkspaceScope::None),
                _ => None,
            };
        }

        // Parse visibility
        if let Some(vis_str) = config_node.visibility {
            ws_config.visibility = match vis_str.to_lowercase().as_str() {
                "public" => Some(WorkspaceVisibility::Public),
                "private" => Some(WorkspaceVisibility::Private),
                _ => None,
            };
        }

        // Parse users
        for user_node in config_node.users {
            let role = match user_node.role.to_lowercase().as_str() {
                "readwrite" | "read-write" | "rw" => UserRole::ReadWrite,
                _ => UserRole::ReadOnly,
            };
            ws_config.users.push(WorkspaceUser::new(user_node.username, role));
        }

        workspace.workspace_configuration = Some(ws_config);
    }

    // Step 6: Store !docs and !adrs paths in workspace properties
    for directive in &ast.directives {
        match directive {
            Directive::Docs(path) => {
                workspace.set_property("structurizr.docs", path);
            }
            Directive::Adrs(path) => {
                workspace.set_property("structurizr.adrs", path);
            }
            _ => {}
        }
    }

    // Build identifier mapping
    let mut identifiers: HashMap<String, ElementId> = HashMap::new();

    // Process model
    if let Some(model_node) = ast.model {
        for element in &model_node.elements {
            if element.kind != ElementKind::DeploymentNode {
                build_element(&mut workspace, element, &mut identifiers)?;
            }
        }

        // Process enterprise elements (marked as internal)
        if let Some(enterprise) = &model_node.enterprise {
            // Set enterprise name on the workspace
            workspace.set_property("structurizr.enterprise", &enterprise.name);

            // Build elements defined inside enterprise (these are internal)
            for element in &enterprise.elements {
                build_element(&mut workspace, element, &mut identifiers)?;
            }

            // Build groups inside enterprise
            for group in &enterprise.groups {
                build_group(&mut workspace, group, &mut identifiers)?;
            }
        }

        // Build groups after all elements exist
        for group in &model_node.groups {
            build_group(&mut workspace, group, &mut identifiers)?;
        }

        for element in &model_node.elements {
            if element.kind == ElementKind::DeploymentNode {
                build_element(&mut workspace, element, &mut identifiers)?;
            }
        }

        // Build deployment environments
        for env in &model_node.deployment_environments {
            for child in &env.children {
                if child.kind == ElementKind::DeploymentNode {
                    let mut node = build_deployment_node(child, workspace.model(), &mut identifiers)?;
                    node.environment = Some(env.name.clone());

                    // Register the node's identifier
                    if let Some(id) = &child.identifier {
                        identifiers.insert(id.clone(), node.id());
                    }

                    workspace.model_mut().deployment_nodes.push(node);
                }
            }
        }

        // Build relationships after all elements exist
        for rel in &model_node.relationships {
            build_relationship(&mut workspace, rel, &identifiers)?;
        }

        // Also process relationships defined inside elements
        for element in &model_node.elements {
            build_element_relationships(&mut workspace, element, &identifiers)?;
        }

        // Step 6: Generate implied relationships if enabled
        if implied_relationships_enabled {
            generate_implied_relationships(&mut workspace);
        }
    }

    // Process views
    if let Some(views_node) = ast.views {
        for view in views_node.system_landscape {
            let properties = build_static_properties(
                workspace.model(),
                ViewType::SystemLandscape,
                None,
                view.properties,
                &identifiers,
                "SystemLandscape".into(),
            )?;
            workspace
                .views_mut()
                .add_system_landscape_view(SystemLandscapeView {
                    properties,
                    enterprise_boundary_visible: true,
                });
        }
        for view in views_node.system_context {
            let system_id = identifiers
                .get(&view.software_system)
                .copied()
                .ok_or_else(|| Error::UndefinedIdentifier(view.software_system.clone()))?;
            let properties = build_static_properties(
                workspace.model(),
                ViewType::SystemContext,
                Some(system_id),
                view.properties,
                &identifiers,
                format!("SystemContext-{}", view.software_system),
            )?;
            workspace
                .views_mut()
                .add_system_context_view(SystemContextView {
                    properties,
                    software_system_id: system_id,
                    enterprise_boundary_visible: true,
                });
        }
        for view in views_node.container {
            let system_id = identifiers
                .get(&view.software_system)
                .copied()
                .ok_or_else(|| Error::UndefinedIdentifier(view.software_system.clone()))?;
            let properties = build_static_properties(
                workspace.model(),
                ViewType::Container,
                Some(system_id),
                view.properties,
                &identifiers,
                format!("Container-{}", view.software_system),
            )?;
            workspace.views_mut().add_container_view(ContainerView {
                properties,
                software_system_id: system_id,
                external_software_system_boundary_visible: true,
            });
        }
        for view in views_node.component {
            let container_id = identifiers
                .get(&view.container)
                .copied()
                .ok_or_else(|| Error::UndefinedIdentifier(view.container.clone()))?;
            let properties = build_static_properties(
                workspace.model(),
                ViewType::Component,
                Some(container_id),
                view.properties,
                &identifiers,
                format!("Component-{}", view.container),
            )?;
            workspace.views_mut().add_component_view(ComponentView {
                properties,
                container_id,
                external_software_system_boundary_visible: true,
            });
        }

        for view in views_node.dynamic {
            let dynamic = build_dynamic_view(workspace.model(), view, &identifiers)?;
            workspace.views_mut().add_dynamic_view(dynamic);
        }

        // Process deployment views
        for view in views_node.deployment {
            let mut v = if let Some(ref scope_id) = view.scope {
                if let Some(&system_id) = identifiers.get(scope_id) {
                    DeploymentView::for_system(
                        view.properties.key.unwrap_or_else(|| format!("Deployment-{}-{}", scope_id, view.environment)),
                        system_id,
                        &view.environment,
                    )
                } else {
                    // Scope system not found - create unscoped view
                    DeploymentView::new(
                        view.properties.key.unwrap_or_else(|| format!("Deployment-{}", view.environment)),
                        &view.environment,
                    )
                }
            } else {
                DeploymentView::new(
                    view.properties.key.unwrap_or_else(|| format!("Deployment-{}", view.environment)),
                    &view.environment,
                )
            };

            // Apply view properties
            if let Some(title) = view.properties.title {
                v.properties = v.properties.with_title(title);
            }
            if let Some(description) = view.properties.description {
                v.properties = v.properties.with_description(description);
            }
            if let Some(auto_layout) = view.properties.auto_layout {
                v.properties = v.properties.with_auto_layout(convert_auto_layout(auto_layout));
            }
            if let Some(background) = view.properties.background {
                v.properties = v.properties.with_background(background);
            }
            if view.properties.default {
                v.properties = v.properties.with_default(true);
            }

            workspace.views_mut().add_deployment_view(v);
        }

        // Process styles
        if let Some(styles_node) = views_node.styles {
            for elem_style in styles_node.elements {
                let mut style = ElementStyle::new(&elem_style.tag);
                if let Some(bg) = elem_style.background {
                    style = style.with_background(bg);
                }
                if let Some(color) = elem_style.color {
                    style = style.with_color(color);
                }
                if let Some(stroke) = elem_style.stroke {
                    style = style.with_stroke(stroke);
                }
                if let Some(font_size) = elem_style.font_size {
                    style = style.with_font_size(font_size);
                }
                if let Some(shape) = elem_style.shape {
                    if let Some(s) = parse_shape(&shape) {
                        style = style.with_shape(s);
                    }
                }
                if let Some(icon) = elem_style.icon {
                    style.icon = Some(icon);
                }
                if let Some(icon_pos) = elem_style.icon_position {
                    if let Some(pos) = parse_icon_position(&icon_pos) {
                        style.icon_position = Some(pos);
                    }
                }
                workspace.styles_mut().add_element_style(style);
            }

            for rel_style in styles_node.relationships {
                let mut style = RelationshipStyle::new(&rel_style.tag);
                if let Some(thickness) = rel_style.thickness {
                    style = style.with_thickness(thickness);
                }
                if let Some(color) = rel_style.color {
                    style = style.with_color(color);
                }
                if let Some(line_style) = rel_style.style {
                    if let Some(s) = parse_line_style(&line_style) {
                        style = style.with_style(s);
                    }
                }
                if let Some(routing) = rel_style.routing {
                    if let Some(r) = parse_routing(&routing) {
                        style = style.with_routing(r);
                    }
                }
                workspace.styles_mut().add_relationship_style(style);
            }
        }

        for theme in views_node.themes {
            workspace.styles_mut().add_theme(theme);
        }

        // Process branding
        if let Some(branding_node) = views_node.branding {
            let mut branding = structurizr_core::style::Branding::default();
            branding.logo = branding_node.logo;
            if let Some(font_node) = branding_node.font {
                branding.font = Some(structurizr_core::style::Font {
                    name: font_node.name,
                    url: font_node.url,
                });
            }
            if let Some(ref mut config) = workspace.configuration {
                config.branding = Some(branding);
            }
        }

        // Process terminology
        if let Some(terminology_node) = views_node.terminology {
            let mut terminology = structurizr_core::style::Terminology::default();
            terminology.enterprise = terminology_node.enterprise;
            terminology.person = terminology_node.person;
            terminology.software_system = terminology_node.software_system;
            terminology.container = terminology_node.container;
            terminology.component = terminology_node.component;
            terminology.deployment_node = terminology_node.deployment_node;
            terminology.infrastructure_node = terminology_node.infrastructure_node;
            terminology.relationship = terminology_node.relationship;
            if let Some(ref mut config) = workspace.configuration {
                config.terminology = Some(terminology);
            }
        }

        for view in views_node.custom {
            let properties = build_static_properties(
                workspace.model(),
                ViewType::Custom,
                None,
                view.properties,
                &identifiers,
                "Custom".into(),
            )?;
            workspace.views_mut().custom_views.push(CustomView { properties });
        }

        // Process image views
        for view in views_node.image {
            // Resolve element identifier to element ID
            let element_id = view.element.and_then(|elem_ident| {
                identifiers.get(&elem_ident).map(|id| id.to_string())
            });

            let mut v = ImageView {
                properties: ViewProperties::new(
                    view.properties.key.unwrap_or_else(|| "Image".to_string()),
                ),
                element_id,
                content: view.content,
                content_type: view.content_type,
            };
            if let Some(title) = view.properties.title {
                v.properties = v.properties.with_title(title);
            }
            if let Some(description) = view.properties.description {
                v.properties = v.properties.with_description(description);
            }
            workspace.views_mut().image_views.push(v);
        }
    }

    Ok(workspace)
}

fn build_element(
    workspace: &mut Workspace,
    element: &ElementNode,
    identifiers: &mut HashMap<String, ElementId>,
) -> Result<ElementId> {
    let id = match element.kind {
        ElementKind::Person => {
            let mut person = Person::new(&element.name);
            if let Some(ref desc) = element.description {
                person = person.with_description(desc);
            }
            let id = person.id();
            workspace.model_mut().people.push(person);
            id
        }
        ElementKind::SoftwareSystem => {
            let mut system = SoftwareSystem::new(&element.name);
            if let Some(ref desc) = element.description {
                system = system.with_description(desc);
            }

            // Process child containers
            for child in &element.children {
                if child.kind == ElementKind::Container {
                    let container_id = build_container(&mut system, child, identifiers)?;
                    if let Some(ref child_id) = child.identifier {
                        identifiers.insert(child_id.clone(), container_id);
                    }
                }
            }

            let id = system.id();
            workspace.model_mut().software_systems.push(system);
            id
        }
        ElementKind::DeploymentNode => {
            let node = build_deployment_node(element, workspace.model(), identifiers)?;
            let id = node.id();
            workspace.model_mut().deployment_nodes.push(node);
            id
        }
        ElementKind::CustomElement => {
            // For customElement, the technology field holds the metadata type
            let metadata = element.technology.clone().unwrap_or_default();
            let mut custom = CustomElement::new(&element.name, metadata);
            if let Some(ref desc) = element.description {
                custom = custom.with_description(desc);
            }
            custom.properties.tags.extend(element.tags.iter().cloned());
            custom.properties.properties = element.properties.clone();
            let id = custom.id();
            workspace.model_mut().custom_elements.push(custom);
            id
        }
        _ => {
            return Err(Error::Parse(ParseError {
                message: format!("Unexpected element kind at top level: {:?}", element.kind),
                location: None,
            }));
        }
    };

    if let Some(ref identifier) = element.identifier {
        identifiers.insert(identifier.clone(), id);
    }

    Ok(id)
}

fn build_group(
    workspace: &mut Workspace,
    group: &GroupNode,
    identifiers: &mut HashMap<String, ElementId>,
) -> Result<()> {
    let mut element_ids = Vec::new();

    // Build elements within the group and collect their IDs
    for element in &group.elements {
        let id = build_element(workspace, element, identifiers)?;
        element_ids.push(id);
    }

    // Recursively handle nested groups
    for nested_group in &group.groups {
        build_group(workspace, nested_group, identifiers)?;
    }

    // Add the group to the model
    if !element_ids.is_empty() {
        let mut model_group = structurizr_core::model::Group::new(&group.name);
        model_group.element_ids = element_ids;
        workspace.model_mut().groups.push(model_group);
    }

    Ok(())
}

fn build_container(
    system: &mut SoftwareSystem,
    element: &ElementNode,
    identifiers: &mut HashMap<String, ElementId>,
) -> Result<ElementId> {
    let mut container = Container::new(&element.name);
    if let Some(ref desc) = element.description {
        container = container.with_description(desc);
    }
    if let Some(ref tech) = element.technology {
        container = container.with_technology(tech);
    }

    // Process child components
    for child in &element.children {
        if child.kind == ElementKind::Component {
            let mut component = Component::new(&child.name);
            if let Some(ref desc) = child.description {
                component = component.with_description(desc);
            }
            if let Some(ref tech) = child.technology {
                component = component.with_technology(tech);
            }
            let comp_id = component.id();
            container.add_component(component);

            if let Some(ref child_id) = child.identifier {
                identifiers.insert(child_id.clone(), comp_id);
            }
        }
    }

    let id = container.id();
    system.add_container(container);
    Ok(id)
}

fn build_deployment_node(
    element: &ElementNode,
    model: &Model,
    identifiers: &mut HashMap<String, ElementId>,
) -> Result<DeploymentNode> {
    let mut node = DeploymentNode::new(&element.name);

    if let Some(ref desc) = element.description {
        node.properties.description = Some(desc.clone());
    }
    if let Some(ref tech) = element.technology {
        node.technology = Some(tech.clone());
    }
    if let Some(ref instances) = element.instances {
        node.instances = instances.clone();
    }

    // Add tags
    for tag in &element.tags {
        if !node.properties.tags.contains(tag) {
            node.properties.tags.push(tag.clone());
        }
    }

    // Add properties
    for (key, value) in &element.properties {
        node.properties.properties.insert(key.clone(), value.clone());
    }

    // Register this node's identifier
    if let Some(ref id) = element.identifier {
        identifiers.insert(id.clone(), node.id());
    }

    // Process children
    for child in &element.children {
        match child.kind {
            ElementKind::DeploymentNode => {
                let child_node = build_deployment_node(child, model, identifiers)?;
                node.children.push(child_node);
            }
            ElementKind::InfrastructureNode => {
                let mut infra = InfrastructureNode::new(&child.name);
                if let Some(ref desc) = child.description {
                    infra.properties.description = Some(desc.clone());
                }
                if let Some(ref tech) = child.technology {
                    infra.technology = Some(tech.clone());
                }
                for tag in &child.tags {
                    if !infra.properties.tags.contains(tag) {
                        infra.properties.tags.push(tag.clone());
                    }
                }
                infra.properties.properties = child.properties.clone();
                if let Some(ref id) = child.identifier {
                    identifiers.insert(id.clone(), infra.id());
                }
                node.infrastructure_nodes.push(infra);
            }
            ElementKind::ContainerInstance | ElementKind::SoftwareSystemInstance => {
                build_deployment_instance(&mut node, child, model, identifiers)?;
            }
            _ => {}
        }
    }

    Ok(node)
}

/// Resolve an instance to a real, correctly typed declaration. Each placement
/// has its own identity even when multiple nodes instantiate the same element.
fn build_deployment_instance(
    node: &mut DeploymentNode,
    child: &ElementNode,
    model: &Model,
    identifiers: &mut HashMap<String, ElementId>,
) -> Result<()> {
    let base_id = *identifiers
        .get(&child.name)
        .ok_or_else(|| Error::UndefinedIdentifier(child.name.clone()))?;
    let base = model
        .find_element(base_id)
        .ok_or_else(|| Error::UndefinedIdentifier(child.name.clone()))?;
    let is_container = match (child.kind, base) {
        (ElementKind::ContainerInstance, ElementRef::Container(_)) => true,
        (ElementKind::SoftwareSystemInstance, ElementRef::SoftwareSystem(_)) => false,
        _ => {
            return Err(Error::Parse(ParseError {
                message: format!(
                    "Invalid model type for deployment instance '{}'",
                    child.name
                ),
                location: None,
            }))
        }
    };
    let id = ElementId::new();
    let mut tags = base.properties().tags.clone();
    tags.push(
        if is_container {
            "Container Instance"
        } else {
            "Software System Instance"
        }
        .into(),
    );
    tags.extend(child.tags.iter().cloned());
    let health_checks = child
        .health_checks
        .iter()
        .map(|source| {
            let mut check = HealthCheck::new(&source.name, &source.url);
            if let Some(interval) = source.interval {
                check.interval = interval;
            }
            if let Some(timeout) = source.timeout {
                check.timeout = timeout;
            }
            check
        })
        .collect();
    if is_container {
        node.container_instances.push(ContainerInstance {
            id,
            container_id: base_id,
            instance_id: 1,
            description: child.description.clone(),
            tags,
            properties: child.properties.clone(),
            health_checks,
        });
    } else {
        node.software_system_instances.push(SoftwareSystemInstance {
            id,
            software_system_id: base_id,
            instance_id: 1,
            description: child.description.clone(),
            tags,
            properties: child.properties.clone(),
            health_checks,
        });
    }
    if let Some(name) = &child.identifier {
        identifiers.insert(name.clone(), id);
    }
    Ok(())
}

fn build_relationship(
    workspace: &mut Workspace,
    rel: &RelationshipNode,
    identifiers: &HashMap<String, ElementId>,
) -> Result<()> {
    let source_id = identifiers.get(&rel.source).ok_or_else(|| {
        Error::UndefinedIdentifier(rel.source.clone())
    })?;
    let dest_id = identifiers.get(&rel.destination).ok_or_else(|| {
        Error::UndefinedIdentifier(rel.destination.clone())
    })?;

    workspace.model_mut().add_relationship_with_metadata(
        *source_id,
        *dest_id,
        rel.description.clone().unwrap_or_default(),
        rel.technology.clone(),
        rel.tags.clone(),
        rel.properties.clone(),
        rel.perspectives.clone(),
    );

    Ok(())
}

fn build_element_relationships(
    workspace: &mut Workspace,
    element: &ElementNode,
    identifiers: &HashMap<String, ElementId>,
) -> Result<()> {
    // Process relationships defined inside this element
    // The relationship's source is stored in rel.source, not the element's identifier
    for rel in &element.relationships {
        if let Some(&source_id) = identifiers.get(&rel.source) {
            if let Some(&dest_id) = identifiers.get(&rel.destination) {
                workspace.model_mut().add_relationship_with_metadata(
                    source_id,
                    dest_id,
                    rel.description.clone().unwrap_or_default(),
                    rel.technology.clone(),
                    rel.tags.clone(),
                    rel.properties.clone(),
                    rel.perspectives.clone(),
                );
            }
        }
    }

    // Recurse into children
    for child in &element.children {
        build_element_relationships(workspace, child, identifiers)?;
    }

    Ok(())
}

/// Resolve a dynamic scope and every step before publishing a complete view.
fn build_dynamic_view(
    model: &Model,
    source: DynamicViewNode,
    identifiers: &HashMap<String, ElementId>,
) -> Result<DynamicView> {
    let scope = source
        .scope
        .as_ref()
        .map(|name| {
            identifiers
                .get(name)
                .copied()
                .ok_or_else(|| Error::UndefinedIdentifier(name.clone()))
        })
        .transpose()?;
    let kind = match scope.map(|id| model.find_element(id)) {
        None => ViewType::SystemLandscape,
        Some(Some(ElementRef::SoftwareSystem(_))) => ViewType::Container,
        Some(Some(ElementRef::Container(_))) => ViewType::Component,
        _ => {
            return Err(Error::Parse(ParseError {
                message: "Invalid dynamic view scope".into(),
                location: None,
            }))
        }
    };
    let (properties, selection) = build_view_properties(source.properties, "Dynamic".into());
    if !selection.is_empty() {
        return Err(Error::Parse(ParseError {
            message: "Dynamic views select interactions, not include/exclude terms".into(),
            location: None,
        }));
    }
    let mut steps = Vec::new();
    flatten_dynamic_sequence(
        &source.content,
        identifiers,
        model,
        scope,
        kind,
        0,
        &mut steps,
    )?;
    let members: HashSet<_> = steps
        .iter()
        .flat_map(|step| [step.source_id, step.destination_id])
        .collect();
    for id in &members {
        if model
            .ancestor_ids(*id)
            .iter()
            .any(|parent| members.contains(parent))
        {
            return Err(Error::Parse(ParseError {
                message: "A dynamic view cannot contain both an element and its ancestor".into(),
                location: None,
            }));
        }
    }
    Ok(DynamicView {
        properties,
        element_id: scope,
        steps,
    })
}

/// Visit each authored sequence. Nested parallel branches share their parent's
/// starting ordinal; an outer grouping block advances the following sequence.
fn flatten_dynamic_sequence(
    content: &[DynamicContent],
    identifiers: &HashMap<String, ElementId>,
    model: &Model,
    scope: Option<ElementId>,
    kind: ViewType,
    mut ordinal: u32,
    steps: &mut Vec<DynamicViewStep>,
) -> Result<(u32, bool)> {
    let mut direct_steps = false;
    for item in content {
        match item {
            DynamicContent::Step(step) => {
                ordinal += 1;
                direct_steps = true;
                let order = step.order.clone().unwrap_or_else(|| ordinal.to_string());
                append_dynamic_step(model, identifiers, kind, scope, step, order, steps)?;
            }
            DynamicContent::ParallelSequence(branch) => {
                let (end, direct) = flatten_dynamic_sequence(
                    &branch.content,
                    identifiers,
                    model,
                    scope,
                    kind,
                    ordinal,
                    steps,
                )?;
                if !direct {
                    ordinal = end + 1;
                }
            }
        }
    }
    Ok((ordinal, direct_steps))
}

/// Bind an interaction to its actual model declaration, including technology
/// disambiguation and replies to an earlier request. Never drop unknown endpoints.
fn append_dynamic_step(
    model: &Model,
    identifiers: &HashMap<String, ElementId>,
    kind: ViewType,
    scope: Option<ElementId>,
    step: &DynamicStepNode,
    order: String,
    steps: &mut Vec<DynamicViewStep>,
) -> Result<()> {
    let source = resolve_view_element(model, kind, scope, &step.source, identifiers)?;
    let destination = resolve_view_element(model, kind, scope, &step.destination, identifiers)?;
    let forward = matching_dynamic_relationship(model, source, destination, step);
    let reverse = || {
        model.relationships.iter().find(|relationship| {
            relationship.source_id == destination
                && relationship.destination_id == source
                && step.technology.is_none()
                && steps
                    .iter()
                    .any(|previous| previous.relationship_id == Some(relationship.id))
        })
    };
    let relationship = forward.or_else(reverse).ok_or_else(|| {
        Error::Parse(ParseError {
            message: format!(
                "No matching model relationship or earlier request for '{}' -> '{}'",
                step.source, step.destination
            ),
            location: None,
        })
    })?;
    steps.push(DynamicViewStep {
        source_id: source,
        destination_id: destination,
        description: step
            .description
            .clone()
            .filter(|description| !description.is_empty())
            .or_else(|| relationship.description.clone()),
        order,
        relationship_id: Some(relationship.id),
        response: relationship.source_id != source,
    });
    Ok(())
}

/// Prefer the authored description among matching forward declarations, falling
/// back to the first technology-compatible declaration without allocating a list.
fn matching_dynamic_relationship<'a>(
    model: &'a Model,
    source: ElementId,
    destination: ElementId,
    step: &DynamicStepNode,
) -> Option<&'a Relationship> {
    let mut candidates = model.relationships.iter().filter(|relationship| {
        relationship.source_id == source
            && relationship.destination_id == destination
            && step
                .technology
                .as_ref()
                .is_none_or(|technology| relationship.technology.as_ref() == Some(technology))
    });
    let first = candidates.next()?;
    if step.description.is_none() || first.description == step.description {
        return Some(first);
    }
    candidates
        .find(|relationship| relationship.description == step.description)
        .or(Some(first))
}

fn convert_auto_layout(node: AutoLayoutNode) -> AutoLayout {
    // Use AutoLayout's defaults when not specified in DSL
    let default_layout = AutoLayout::default();

    AutoLayout {
        direction: match node.direction {
            crate::ast::AutoLayoutDirection::TopBottom => structurizr_core::view::AutoLayoutDirection::TopBottom,
            crate::ast::AutoLayoutDirection::BottomTop => structurizr_core::view::AutoLayoutDirection::BottomTop,
            crate::ast::AutoLayoutDirection::LeftRight => structurizr_core::view::AutoLayoutDirection::LeftRight,
            crate::ast::AutoLayoutDirection::RightLeft => structurizr_core::view::AutoLayoutDirection::RightLeft,
        },
        rank_separation: node.rank_separation.unwrap_or(default_layout.rank_separation),
        node_separation: node.node_separation.unwrap_or(default_layout.node_separation),
    }
}

fn parse_shape(s: &str) -> Option<Shape> {
    match s.to_lowercase().as_str() {
        "box" => Some(Shape::Box),
        "roundedbox" => Some(Shape::RoundedBox),
        "circle" => Some(Shape::Circle),
        "ellipse" => Some(Shape::Ellipse),
        "hexagon" => Some(Shape::Hexagon),
        "cylinder" => Some(Shape::Cylinder),
        "pipe" => Some(Shape::Pipe),
        "person" => Some(Shape::Person),
        "robot" => Some(Shape::Robot),
        "folder" => Some(Shape::Folder),
        "webbrowser" => Some(Shape::WebBrowser),
        "mobiledeviceportrait" => Some(Shape::MobileDevicePortrait),
        "mobiledevicelandscape" => Some(Shape::MobileDeviceLandscape),
        "component" => Some(Shape::Component),
        _ => None,
    }
}

fn parse_line_style(s: &str) -> Option<LineStyle> {
    match s.to_lowercase().as_str() {
        "solid" => Some(LineStyle::Solid),
        "dashed" => Some(LineStyle::Dashed),
        "dotted" => Some(LineStyle::Dotted),
        _ => None,
    }
}

fn parse_routing(s: &str) -> Option<Routing> {
    match s.to_lowercase().as_str() {
        "direct" => Some(Routing::Direct),
        "orthogonal" => Some(Routing::Orthogonal),
        "curved" => Some(Routing::Curved),
        _ => None,
    }
}

fn parse_icon_position(s: &str) -> Option<IconPosition> {
    match s.to_lowercase().as_str() {
        "top" => Some(IconPosition::Top),
        "bottom" => Some(IconPosition::Bottom),
        "left" => Some(IconPosition::Left),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_workspace() {
        let dsl = r#"
workspace "Test" "A test workspace" {
    model {
        user = person "User" "A user"
        system = softwareSystem "System" "A system"
        user -> system "Uses"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        assert_eq!(workspace.name, "Test");
        assert_eq!(workspace.model().people.len(), 1);
        assert_eq!(workspace.model().software_systems.len(), 1);
        assert_eq!(workspace.model().relationships.len(), 1);
    }

    #[test]
    fn test_parse_with_views() {
        let dsl = r#"
workspace {
    model {
        user = person "User"
        system = softwareSystem "System"
    }
    views {
        systemContext system {
            include *
            autoLayout
        }
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        assert_eq!(workspace.views().system_context_views.len(), 1);
    }

    #[test]
    fn test_const_directive() {
        let dsl = r#"
workspace "Test" {
    !const COMPANY_NAME "Acme Corp"
    !const TECH_STACK "Java/Spring"

    model {
        user = person "${COMPANY_NAME} User" "A user"
        system = softwareSystem "System" "Built with ${TECH_STACK}"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        assert_eq!(workspace.model().people.len(), 1);
        assert_eq!(workspace.model().people[0].name(), "Acme Corp User");
        assert_eq!(workspace.model().software_systems.len(), 1);
        assert_eq!(workspace.model().software_systems[0].properties.description, Some("Built with Java/Spring".to_string()));
    }

    #[test]
    fn test_implied_relationships() {
        let dsl = r#"
workspace "Test" {
    !impliedRelationships true

    model {
        a = person "A"
        b = softwareSystem "B"
        c = softwareSystem "C"

        a -> b "Uses"
        b -> c "Calls"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        // A graph chain does not create a transitive A->C relationship.
        assert_eq!(workspace.model().relationships.len(), 2);
    }

    #[test]
    fn test_docs_adrs_directives() {
        let dsl = r#"
workspace "Test" {
    !docs "docs"
    !adrs "decisions"

    model {
        user = person "User"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        assert_eq!(workspace.get_property("structurizr.docs"), Some(&"docs".to_string()));
        assert_eq!(workspace.get_property("structurizr.adrs"), Some(&"decisions".to_string()));
    }

    #[test]
    fn test_implied_relationships_disabled() {
        let dsl = r#"
workspace "Test" {
    !impliedRelationships false

    model {
        a = person "A"
        b = softwareSystem "B"
        c = softwareSystem "C"

        a -> b "Uses"
        b -> c "Calls"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        // Should have only 2 relationships: A->B, B->C
        assert_eq!(workspace.model().relationships.len(), 2);
    }

    #[test]
    fn test_const_substitution_in_relationships() {
        let dsl = r#"
workspace "Test" {
    !const PROTOCOL "HTTPS"

    model {
        a = person "A"
        b = softwareSystem "B"

        a -> b "Uses" "${PROTOCOL}"
    }
}
"#;
        let workspace = parse(dsl).unwrap();
        assert_eq!(workspace.model().relationships.len(), 1);
        assert_eq!(workspace.model().relationships[0].technology, Some("HTTPS".to_string()));
    }

    #[test]
    fn test_script_directive_inline() {
        let dsl = r#"
workspace "Test" {
    !script lua {
        workspace:setName("Modified")
    }
    model {
    }
}
"#;
        let ast = parse_to_ast(dsl).unwrap();
        // Check that script directive was parsed
        let has_script = ast.directives.iter().any(|d| matches!(d, Directive::Script(_)));
        assert!(has_script, "Should have parsed script directive");

        if let Some(Directive::Script(script)) = ast.directives.iter().find(|d| matches!(d, Directive::Script(_))) {
            assert_eq!(script.language, Some("lua".to_string()));
            assert!(script.content.is_some());
            assert!(script.file_path.is_none());
        }
    }

    #[test]
    fn test_script_directive_file() {
        let dsl = r#"
workspace "Test" {
    !script "scripts/modify.lua"
    model {
    }
}
"#;
        let ast = parse_to_ast(dsl).unwrap();
        // Check that script directive was parsed
        let has_script = ast.directives.iter().any(|d| matches!(d, Directive::Script(_)));
        assert!(has_script, "Should have parsed script directive");

        if let Some(Directive::Script(script)) = ast.directives.iter().find(|d| matches!(d, Directive::Script(_))) {
            assert!(script.language.is_none());
            assert!(script.content.is_none());
            assert_eq!(script.file_path, Some("scripts/modify.lua".to_string()));
        }
    }

    #[test]
    fn test_plugin_directive() {
        let dsl = r#"
workspace "Test" {
    !plugin com.example.MyPlugin
    model {
    }
}
"#;
        let ast = parse_to_ast(dsl).unwrap();
        // Check that plugin directive was parsed
        let has_plugin = ast.directives.iter().any(|d| matches!(d, Directive::Plugin(_)));
        assert!(has_plugin, "Should have parsed plugin directive");
    }
}
