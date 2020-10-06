use yaml_rust::Yaml;
use yaml_variable_substitution::*;
use abstract_pipeline_runner::*;

fn load_yaml_from_file_with_context(
    file_path: &str,
    context: Vec<String>
) -> Result<Vec<Yaml>, String> {
    let ydocs = read_yaml_from_file(file_path, context);
    if let Err(ref err) = ydocs {
        return Err(err.to_string());
    }

    let ydocs = ydocs.unwrap();
    if ydocs.len() < 1 {
        return Err(format!("Failed to parse yaml file {}", file_path));
    }

    Ok(ydocs)
}

pub struct ShellTask {}
impl Task for ShellTask {
    fn run(&self, node_task: &Node, global_context: &GlobalContext)
    -> (bool, Option<Vec<ContextDiff>>) {
        // TODO: implement running a node's task string via shell command
        (false, None)
    }
}

fn yaml_hash_has_key(yaml: &Yaml, key: &str) -> bool {
    match yaml {
        Yaml::Hash(h) => h.keys().any(|k| k.as_str() == Some(key)),
        _ => false,
    }
}


#[derive(PartialEq, Copy, Clone)]
pub enum ParserNodeType {
    ParserNodeTypeSeries,
    ParserNodeTypeParallel,
    ParserNodeTypeTask,
    ParserNodeTypeKnown,
}
pub use ParserNodeType::*;
pub trait Parser {
    fn get_node_type(&self) -> ParserNodeType;
    fn create_task_node<'a>(&'a self, task: &'a dyn Task) -> Option<Node>;
    fn collect_node_vec<'a>(&'a self, task: &'a dyn Task, node_type: ParserNodeType) -> Vec<Node>;

    fn kwd_name(&self) -> &str { "name" }
    fn kwd_series(&self) -> &str { "series" }
    fn kwd_parallel(&self) -> &str { "parallel" }
    fn kwd_task(&self) -> &str { "task" }
    fn get_node_name<'a>(&'a self) -> Option<&'a str> { None }
    fn make_node<'a>(&'a self, task: &'a dyn Task) -> Option<Node> {
        let node_type = self.get_node_type();
        if node_type == ParserNodeTypeParallel || node_type == ParserNodeTypeSeries {
            let mut node = Node::default();
            node.name = self.get_node_name();
            let node_vec = self.collect_node_vec(task, node_type);
            node.ntype = if node_type == ParserNodeTypeParallel {
                NodeTypeParallel(node_vec)
            } else {
                // otherwhise its series
                NodeTypeSeries(node_vec)
            };
            return Some(node);
        }

        if node_type == ParserNodeTypeTask {
            return self.create_task_node(task);
        }

        // TODO: also implement adding a known node
        None
    }
}
impl Parser for Yaml {
    fn get_node_type(&self) -> ParserNodeType {
        if yaml_hash_has_key(self, self.kwd_series()) {
            ParserNodeTypeSeries
        } else if yaml_hash_has_key(self, self.kwd_parallel()) {
            ParserNodeTypeParallel
        } else if yaml_hash_has_key(self, self.kwd_task()) {
            ParserNodeTypeTask
        } else if let Yaml::String(_) = self {
            // if its just a single string, it's probably
            // a task
            ParserNodeTypeTask
        } else {
            ParserNodeTypeKnown
        }
    }

    fn get_node_name<'a>(&'a self) -> Option<&'a str> {
        if let Yaml::Hash(h) = self {
            if yaml_hash_has_key(self, self.kwd_name()) {
                for (k, v) in h {
                    match (k.as_str(), v.as_str()) {
                        (Some(key), Some(value)) => {
                            if key == self.kwd_name() {
                                return Some(value);
                            }
                        },
                        _ => (),
                    }
                }
            }
        }
        None
    }

    fn collect_node_vec<'a>(&'a self, task: &'a dyn Task, node_type: ParserNodeType) -> Vec<Node<'a>> {
        let kwd = if node_type == ParserNodeTypeParallel {
            self.kwd_parallel()
        } else if node_type == ParserNodeTypeSeries {
            self.kwd_series()
        } else {
            panic!("unsupported usage")
        };

        let mut node_vec = vec![];
        if let Yaml::Array(yaml_array) = &self[kwd] {
            for yaml_obj in yaml_array {
                let node_obj = yaml_obj.make_node(task);
                if node_obj.is_some() {
                    node_vec.push(node_obj.unwrap());
                }
            }
        }
        node_vec
    }
    fn create_task_node<'a>(&'a self, task: &'a dyn Task) -> Option<Node> {
        if let Yaml::Hash(h) = self {
            let mut node = Node::default();
            node.ntype = NodeTypeTask;
            for (k, v) in h {
                match (k.as_str(), v.as_str()) {
                    (Some(key), Some(value)) => {
                        if key == self.kwd_name() {
                            node.name = Some(value);
                        } else {
                            node.properties.insert(key, value);
                        }
                    },
                    _ => (),
                }
            }
            node.task = Some(task);
            return Some(node);
        } else if let Yaml::String(s) = self {
            let mut node = Node::default();
            node.ntype = NodeTypeTask;
            node.properties.insert(self.kwd_task(), s);
            node.task = Some(task);
            return Some(node);
        }
        None
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let yaml_path = &args[1];
    let context = &args[2..];
    let context_vec = context.to_vec();
    let yaml_vec = load_yaml_from_file_with_context(yaml_path, context_vec);
    if let Err(yaml_error) = yaml_vec {
        println!("{}", yaml_error);
        std::process::exit(1);
    }

    let yaml_vec = yaml_vec.unwrap();
    let yaml = &yaml_vec[0];
    println!("MY YAML: {:?}", yaml);

    let mut task = ShellTask {};
    let mut global_context = GlobalContext::default();
    let mut root_node = yaml.make_node(&task);

    if root_node.is_none() {
        println!("Failed to create node hierarchy from yaml");
        std::process::exit(1);
    }
    let mut root_node = root_node.unwrap();
    println!("{}", root_node.pretty_print());
}
