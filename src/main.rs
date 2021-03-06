use yaml_rust::Yaml;
use yaml_variable_substitution::*;
use context_based_variable_substitution::*;
use abstract_pipeline_runner::*;
use abstract_pipeline_parsers::*;
use abstract_pipeline_parsers::parsers::yaml::*;
use std::collections::HashMap;
use std::process::Command;

// TODO: use these and make nice output :)
use ansi_term::Colour::Yellow;
use ansi_term::Colour::Red;
use ansi_term::Colour::Green;

pub const KWD_TASK: &str = "run";
pub const KWD_ENV: &str = "env";
pub const KWD_CAP_STDOUT: &str = "capture_stdout";
pub const KWD_CAP_STDERR: &str = "capture_stderr";
pub const KWD_DISPLAY: &str = "display";

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

pub fn fill_all_node_properties<U: Task<Property> + Clone>(
    node: &mut Node<Property, U>,
    context: &impl Context,
) {
    match node.ntype {
        NodeTypeTask => {
            let mut prop_keys = vec![];
            for key in node.properties.keys() {
                prop_keys.push(*key);
            }
            for key in prop_keys {
                let prop = &node.properties[key];
                let new_prop = replace_property_with_context(prop, context);
                node.properties.insert(key, new_prop);
            }
        },
        NodeTypeSeries(ref mut node_vec) => {
            for n in node_vec {
                fill_all_node_properties(n, context);
            }
        },
        NodeTypeParallel(ref mut node_vec) => {
            for n in node_vec {
                fill_all_node_properties(n, context);
            }
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
        // task display will default to being the name of the task
        let mut task_display = match node_task.name {
            Some(s) => Some(s.to_string()),
            None => None,
        };
        let gc_holder = GCHolder { gc: global_context };
        for (key, prop) in &node_task.properties {
            let replaced_prop = replace_property_with_context(prop, &gc_holder);
            if *key == KWD_ENV {
                if let Property::Map(m) = replaced_prop {
                    for (env_key, env_val) in m {
                        if let Property::Simple(s) = env_val {
                            env_keys.push(env_key.into());
                            env_vals.push(s);
                        }
                    }
                }
            } else if *key == KWD_TASK {
                if let Property::Simple(s) = replaced_prop {
                    // if there wasnt a task name, try setting it
                    // to the actual string of the task
                    if task_display.is_none() {
                        task_display = Some(s.clone());
                    }
                    cmd_str = Some(s);
                }
            } else if *key == KWD_CAP_STDOUT {
                if let Property::Simple(s) = replaced_prop {
                    capture_stdout = Some(s);
                }
            } else if *key == KWD_CAP_STDERR {
                if let Property::Simple(s) = replaced_prop {
                    capture_stderr = Some(s);
                }
            } else if *key == KWD_DISPLAY {
                // no matter what, if there's an explicit
                // display key, use the display value
                if let Property::Simple(s) = replaced_prop {
                    task_display = Some(s);
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
                    fill_all_node_properties(&mut node_clone, &current_node_context);

                    // for this implementation of a pipeline runner,
                    // I thought it'd be nice to display the original call
                    // for the known_node, so we explicitly give this new node
                    // a display: (this will only work if the node_clone is
                    // a task node. otherwise, it does not make sense to iterate
                    // over all its children and giving all of them a display prop)
                    let display_text = cmd_str;
                    node_clone.properties.insert(KWD_DISPLAY, Property::Simple(display_text.into()));

                    // then return from the current task by calling the
                    // run node helper seperately. this will still return
                    // a success code and a list of diffs depending on what
                    // the known node does
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
            }
        }
        let diff_vec_opt = if diff_vec.len() > 0 { Some(diff_vec) } else { None };
        if let Some(task_name) = task_display {
            let color_text = match success {
                true => Green.paint(task_name),
                false => Red.paint(task_name),
            };
            println!("{}", color_text);
        }
        (success, diff_vec_opt)
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
