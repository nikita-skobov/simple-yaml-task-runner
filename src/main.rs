use yaml_rust::Yaml;
use yaml_variable_substitution::*;
use context_based_variable_substitution::*;
use abstract_pipeline_runner::*;
use std::collections::HashMap;
use std::process::Command;

// TODO: use these and make nice output :)
use ansi_term::Colour::Yellow;
use ansi_term::Colour::Red;
use ansi_term::Colour::Green;

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

pub struct GCHolder<'a, T: Send + Sync + Clone, U: Task<T> + Clone> {
    pub gc: &'a GlobalContext<'a, T, U>
}
impl<'a, T: Send + Sync + Clone, U: Task<T> + Clone> Context for GCHolder<'a, T, U> {
    fn get_value_from_key(&self, key: &str, syntax_char: char) -> Option<String> {
        if self.gc.variables.contains_key(key) {
            Some(self.gc.variables[key].clone())
        } else {
            None
        }
    }
}

pub struct NodeContext<'a, T: Send + Sync + Clone, U: Task<T> + Clone> {
    pub gc_holder: GCHolder<'a, T, U>,
    pub cmd_list: Vec<&'a str>,
}
impl<'a, T: Send + Sync + Clone, U: Task<T> + Clone> Context for NodeContext<'a, T, U> {
    fn get_value_from_key(&self, key: &str, syntax_char: char) -> Option<String> {
        // try first to get key from the cmd_list
        let value_from_cmd = self.cmd_list.get_value_from_key(key, syntax_char);

        match value_from_cmd {
            Some(_) => value_from_cmd,
            // if not found, default back to the global context
            None => self.gc_holder.get_value_from_key(key, syntax_char),
        }
    }
}

// take a reference to a real property
// and return a property that has all
// of its fields replaced via a provided context
pub fn replace_property_with_context(
    prop: &Property,
    context: &impl Context,
) -> Property {
    match prop {
        Property::Simple(s) => {
            Property::Simple(
                replace_all_from(
                    s,
                    context,
                    FailureMode::FM_ignore,
                    Some("?"),
                )
            )
        }
        Property::Map(m) => {
            let mut new_hashmap = HashMap::new();
            for (k, v) in m {
                new_hashmap.insert(
                    k.into(),
                    replace_property_with_context(v, context),
                );
            }
            Property::Map(new_hashmap)
        }
    }
}

#[derive(Clone)]
pub struct ShellTask {}
impl Task<Property> for ShellTask {
    fn run<U: Task<Property> + Clone>(
        &self,
        node_task: &Node<Property, U>,
        global_context: &GlobalContext<Property, U>,
    ) -> (bool, Option<Vec<ContextDiff>>)
    {
        let mut env_keys = vec![];
        let mut env_vals = vec![];
        let mut cmd_str = None;
        let mut capture_stdout = None;
        let mut capture_stderr = None;
        let gc_holder = GCHolder { gc: global_context };
        for (key, prop) in &node_task.properties {
            let replaced_prop = replace_property_with_context(prop, &gc_holder);
            if *key == "env" {
                if let Property::Map(m) = replaced_prop {
                    for (env_key, env_val) in m {
                        if let Property::Simple(s) = env_val {
                            env_keys.push(env_key.into());
                            env_vals.push(s);
                        }
                    }
                }
            } else if *key == "task" {
                if let Property::Simple(s) = replaced_prop {
                    cmd_str = Some(s);
                }
            } else if *key == "capture_stdout" {
                if let Property::Simple(s) = replaced_prop {
                    capture_stdout = Some(s);
                }
            } else if *key == "capture_stderr" {
                if let Property::Simple(s) = replaced_prop {
                    capture_stderr = Some(s);
                }
            }
        }

        // before running it as a shell command,
        // check if it exists in the global context known
        // nodes. if so, then run that known node instead
        if let Some(ref cmd_str) = cmd_str {
            let cmd_vec: Vec<&str> = cmd_str.split(" ").collect();
            let cmd_exec = if cmd_vec.len() > 0 { Some(cmd_vec[0]) } else { None };
            if let Some(cmd_exec) = cmd_exec {
                if global_context.known_nodes.contains_key(cmd_exec) {
                    let node = &global_context.known_nodes[cmd_exec];
                    // now that we detected a known node, we use that node
                    // as a template for what we are about to run
                    // first we need to fill in that node template with
                    // the context of the current node we are on
                    let mut node_clone = node.clone();
                    let current_node_context = NodeContext {
                        gc_holder,
                        cmd_list: cmd_vec,
                    };

                    for (o_key, o_prop) in &node.properties {
                        let new_prop = replace_property_with_context(o_prop, &current_node_context);
                        node_clone.properties.insert(o_key, new_prop);
                    }

                    // TODO: make this a function that will visit every node
                    // child of this node_clone, and fill in the properties
                    // not just if its a root task
                    // match node_clone.ntype {
                    //     NodeTypeTask => {
                    //         for (prop_name, mut prop) in node_clone.properties {
                    //             match prop {
                    //                 Property::Simple(mut s) => {
                    //                     s = replace_all_from(
                    //                         s.as_str(),
                    //                         &current_node_context,
                    //                         FailureMode::FM_ignore,
                    //                         Some("?")
                    //                     );
                    //                 }
                    //                 _ => (),
                    //                 // TODO: iterate over this map, and do a
                    //                 // replace for all values
                    //                 // Property::Map(_) => {}
                    //             }
                    //         }
                    //     }
                    //     _ => (),
                    // }

                    return run_node_helper_immut(&node_clone, &global_context);
                }
            }
        }

        let mut diff_vec = vec![];
        let mut success = true;
        if let Some(cmd_str) = cmd_str {
            let (status, stdout, stderr) = exec_shell(
                cmd_str.as_str(), env_keys, env_vals
            );
            if status != 0 {
                success = false;
            }
            if let Some(cap_stderr) = capture_stderr {
                diff_vec.push(ContextDiff::CDSet(cap_stderr.into(), stderr.trim().into()));
            }
            if let Some(cap_stdout) = capture_stdout {
                diff_vec.push(ContextDiff::CDSet(cap_stdout.into(), stdout.trim().into()));
            } else {
                // only print stdout if its not captured
                println!("{}", stdout);
            }
        }
        let diff_vec_opt = if diff_vec.len() > 0 { Some(diff_vec) } else { None };
        (success, diff_vec_opt)
    }
}

fn yaml_hash_has_key(yaml: &Yaml, key: &str) -> bool {
    match yaml {
        Yaml::Hash(h) => h.keys().any(|k| k.as_str() == Some(key)),
        _ => false,
    }
}

fn exec_shell(
    cmd_str: &str,
    env_keys: Vec<String>,
    env_vals: Vec<String>,
) -> (i32, String, String) {
    assert_eq!(env_vals.len(), env_keys.len());

    let mut cmd = Command::new("sh");
    for i in 0..env_keys.len() {
        cmd.env(env_keys[i].as_str(), env_vals[i].as_str());
    }
    cmd.arg("-c").arg(cmd_str);
    let out = cmd.output().expect("something bad");
    let stdout_cow = String::from_utf8_lossy(&out.stdout);
    let stderr_cow = String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(1), stdout_cow.into(), stderr_cow.into())
}


#[derive(PartialEq, Copy, Clone, Debug)]
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
    fn create_task_node<'a, U: Task<T> + Clone>(&'a self, task: &'a U) -> Option<Node<T, U>>;
    fn collect_node_vec<'a, U: Task<T> + Clone>(&'a self, task: &'a U, node_type: ParserNodeType) -> Vec<Node<T, U>>;

    // these are methods you can implement if you
    // want to customize the behavior a little bit
    fn kwd_name(&self) -> &str { "name" }
    fn kwd_series(&self) -> &str { "series" }
    fn kwd_parallel(&self) -> &str { "parallel" }
    fn kwd_task(&self) -> &str { "task" }
    fn get_node_name<'a>(&'a self) -> Option<&'a str> { None }

    // this is a method you should only implement if you want really
    // specific behavior. this default should work well in most cases
    fn make_node<'a, U: Task<T> + Clone>(&'a self, task: &'a U) -> Option<Node<T, U>> {
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

        // Known Nodes at the root should not be handled by the parser.
        // Instead, use a seperate convenience method for collecting
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

    fn collect_node_vec<'a, U: Task<Property> + Clone>(&'a self, task: &'a U, node_type: ParserNodeType) -> Vec<Node<Property, U>> {
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
    fn create_task_node<'a, U: Task<Property> + Clone>(&'a self, task: &'a U) -> Option<Node<Property, U>> {
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
    if let Yaml::Hash(h) = yaml {
        let mut hashmap = HashMap::new();
        for (k, v) in h {
            if let Some(s) = k.as_str() {
                hashmap.insert(s.into(), create_property_from_yaml_hash(v));
            }
        }
        Property::Map(hashmap)
    } else {
        Property::Simple(get_yaml_key_as_string(yaml))
    }
}

fn get_yaml_key_as_string(yaml: &Yaml) -> String {
    match yaml {
        Yaml::Real(s) => s.into(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::String(s) => s.into(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Null => "null".into(),

        // TODO:
        // Yaml::Array(_) => {}
        // Yaml::Hash(_) => {}
        // Yaml::Alias(_) => {}
        // Yaml::BadValue => {}
        _ => "".into(),
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

    let mut task = ShellTask {};
    let mut global_context: GlobalContext<Property, ShellTask> = GlobalContext {
        known_nodes: HashMap::new(),
        variables: HashMap::new(),
    };
    let mut root_node = yaml.make_node(&task);

    // now we make the known nodes
    let parallel_series_or_task = [
        yaml.kwd_parallel(),
        yaml.kwd_series(),
        yaml.kwd_task(),
    ];
    // we assume that the root yaml is a hash
    if let Yaml::Hash(h) = yaml {
        for (k, v) in h {
            let yaml_key = get_yaml_key_as_string(k);
            // if we encounter a parallel, series, or task
            // at the root, then that is not a known node, and that is
            // probably the root node, so we skip it
            if parallel_series_or_task.contains(&yaml_key.as_str()) {
                continue;
            }
            // now we know we have a potential known node, so we check if it
            // contains a valid kwd of task, parallel or series, if not,
            // then this is not a node that can be visited, and is instead,
            // probably some configuration node, or defaults, or something like that
            if v.get_node_type() != ParserNodeTypeKnown {
                let known_node = v.make_node(&task);
                if let Some(known_node) = known_node {
                    global_context.known_nodes.insert(yaml_key, known_node);
                }
            }
        }
    }

    if root_node.is_none() {
        println!("Failed to create node hierarchy from yaml");
        std::process::exit(1);
    }
    // println!("{:?}", global_context.known_nodes);
    let mut root_node = root_node.unwrap();
    let (success, _) = run_node_helper(&root_node, &mut global_context);
    let exit_code = match success {
        true => 0,
        false => 1,
    };
    std::process::exit(exit_code);
    // println!("{}", root_node.pretty_print());
    // println!("{:?}", global_context.variables);
}
