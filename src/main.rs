use yaml_rust::Yaml;
use yaml_variable_substitution::*;
use abstract_pipeline_runner::*;
use std::collections::HashMap;
use std::process::Command;
use std::process::ExitStatus;

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

#[derive(Clone)]
pub struct ShellTask {}
impl Task for ShellTask {
    fn run<T: Send + Sync + Clone, U: Task + Clone>(&self, node_task: &Node<T, U>, global_context: &GlobalContext<T, U>)
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

fn exec_shell(cmd_str: &str) {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmd_str);
    let out = cmd.output().expect("REEEE");
    if out.status.success() {
        let str_cow = String::from_utf8_lossy(&out.stdout);
        println!("{}", str_cow);
    } else {
        let str_cow = String::from_utf8_lossy(&out.stderr);
        println!("{}", str_cow);
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
pub trait Parser<T: Send + Sync + Clone> {
    // these are methods you must implement as a user
    fn get_node_type(&self) -> ParserNodeType;
    fn create_task_node<'a, U: Task + Clone>(&'a self, task: &'a U) -> Option<Node<T, U>>;
    fn collect_node_vec<'a, U: Task + Clone>(&'a self, task: &'a U, node_type: ParserNodeType) -> Vec<Node<T, U>>;

    // these are methods you can implement if you
    // want to customize the behavior a little bit
    fn kwd_name(&self) -> &str { "name" }
    fn kwd_series(&self) -> &str { "series" }
    fn kwd_parallel(&self) -> &str { "parallel" }
    fn kwd_task(&self) -> &str { "task" }
    fn get_node_name<'a>(&'a self) -> Option<&'a str> { None }

    // this is a method you should only implement if you want really
    // specific behavior. this default should work well in most cases
    fn make_node<'a, U: Task + Clone>(&'a self, task: &'a U) -> Option<Node<T, U>> {
        let node_type = self.get_node_type();
        if node_type == ParserNodeTypeParallel || node_type == ParserNodeTypeSeries {
            let mut node = Node {
                name: None,
                is_root_node: false,
                ntype: NodeTypeTask,
                task: None,
                properties: HashMap::new(),
                continue_on_fail: false,
            };
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

        // Known Nodes at the root should not be handles by the parser
        // instead, use a seperate convenience method for collecting
        // known nodes. This is because known nodes do not get put into
        // the node hierarchy, but are rather stored seperately in a global
        // context, to be accessed by any node in the hiearchy as needed
        None
    }
}


#[derive(Clone, Debug)]
pub enum Property {
    Simple(String),
    Map(HashMap<String, Property>)
}
impl Parser<Property> for Yaml {
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

    fn collect_node_vec<'a, U: Task + Clone>(&'a self, task: &'a U, node_type: ParserNodeType) -> Vec<Node<Property, U>> {
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
    fn create_task_node<'a, U: Task + Clone>(&'a self, task: &'a U) -> Option<Node<Property, U>> {
        if let Yaml::Hash(h) = self {
            let mut node = Node {
                name: None,
                is_root_node: false,
                ntype: NodeTypeTask,
                task: None,
                properties: HashMap::new(),
                continue_on_fail: false,
            };
            node.ntype = NodeTypeTask;
            for (k, v) in h {
                if let Some(s) = k.as_str() {
                    if s == self.kwd_name() && v.as_str().is_some() {
                        node.name = Some(v.as_str().unwrap());
                    }
                    let property = create_property_from_yaml_hash(v);
                    node.properties.insert(s.into(), property);
                }
            }
            node.task = Some(task);
            return Some(node);
        } else if let Yaml::String(s) = self {
            let mut node = Node {
                name: None,
                is_root_node: false,
                ntype: NodeTypeTask,
                task: None,
                properties: HashMap::new(),
                continue_on_fail: false,
            };
            node.ntype = NodeTypeTask;
            node.properties.insert(self.kwd_task(), Property::Simple(s.into()));
            node.task = Some(task);
            return Some(node);
        }
        None
    }
}

fn create_property_from_yaml_hash(yaml: &Yaml) -> Property {
    match yaml {
        Yaml::Real(s) => Property::Simple(s.into()),
        Yaml::String(s) => Property::Simple(s.into()),
        Yaml::Boolean(b) => Property::Simple(b.to_string()),
        Yaml::Null => Property::Simple("null".into()),
        Yaml::Hash(h) => {
            let mut hashmap = HashMap::new();
            for (k, v) in h {
                if let Some(s) = k.as_str() {
                    hashmap.insert(s.into(), create_property_from_yaml_hash(v));
                }
            }
            Property::Map(hashmap)
        }
        _ => Property::Simple("".into()),
        // TODO:
        // Yaml::Alias(_) => {}
        // Yaml::Integer(_) => {}
        // Yaml::Array(_) => {}
        // Yaml::BadValue => {}
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
    let mut global_context: GlobalContext<&str, ShellTask> = GlobalContext {
        known_nodes: HashMap::new(),
        variables: HashMap::new(),
    };
    let mut root_node = yaml.make_node(&task);

    if root_node.is_none() {
        println!("Failed to create node hierarchy from yaml");
        std::process::exit(1);
    }
    let mut root_node = root_node.unwrap();
    // println!("{}", pretty_print(&root_node));


    exec_shell("MY_ENV=\"test\" echo yooo: $MY_ENV");
}
