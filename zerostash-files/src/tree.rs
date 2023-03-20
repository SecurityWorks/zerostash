use crate::Entry;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Node {
    File(Entry),
    Directory(Arc<Mutex<BTreeMap<String, Node>>>),
}

impl Default for Node {
    fn default() -> Self {
        Self::Directory(Arc::default())
    }
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub struct Tree(Arc<Mutex<BTreeMap<String, Node>>>);

impl Tree {
    pub fn insert_directory(&mut self, path: &str, node: Option<Node>) {
        let mut current = self.0.clone();

        current = Self::get_or_insert_root(current);
        let (curr, dir_name) = Self::get_or_insert_last_two_nodes(current, path);
        current = curr;

        if let Some(dir) = node {
            current.lock().unwrap().insert(dir_name.to_string(), dir);
        } else {
            current
                .lock()
                .unwrap()
                .insert(dir_name.to_string(), Node::default());
        }
    }

    pub fn insert_file(&mut self, path: &str, file: Entry) {
        let mut current = self.0.clone();

        current = Self::get_or_insert_root(current);

        let (curr, filename) = Self::get_or_insert_last_two_nodes(current, path);
        current = curr;

        current
            .lock()
            .unwrap()
            .insert(filename.to_string(), Node::File(file));
    }

    pub fn remove(&mut self, path: &str) -> Option<Node> {
        let parts = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let parts_len = parts.len() - 1;
        let mut current = self.0.clone();

        if parts.is_empty() {
            current.lock().unwrap().remove("");
        }

        let child = current.lock().unwrap().get("").cloned();

        current = match child {
            Some(Node::Directory(dir)) => dir,
            _ => return None,
        };

        for part in parts.iter().take(parts_len) {
            let child = current.lock().unwrap().get(*part).cloned();

            current = match child {
                Some(Node::Directory(dir)) => dir,
                _ => return None,
            };
        }

        let node_name = parts.last().expect("Path is not valid");
        let mut current = current.lock().unwrap();
        current.remove(&node_name.to_string())
    }

    pub fn get(&self, path: &str) -> Option<Node> {
        let parts = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let mut current = self.0.clone();

        let child = current.lock().unwrap().get("").cloned();

        current = match child {
            Some(Node::Directory(dir)) => dir,
            _ => return None,
        };

        for part in parts {
            let child = current.lock().unwrap().get(part).cloned();

            current = match child {
                Some(Node::Directory(dir)) => dir,
                Some(node) => return Some(node),
                None => return None,
            };
        }

        Some(Node::Directory(current))
    }

    pub fn rename_file(&mut self, path: &str, name: &str) {
        let mut current = self.0.clone();

        current = Self::get_root(current);

        let (curr, filename) = Self::get_last_two_nodes(current, path);
        current = curr;
        let mut lock = current.lock().unwrap();
        let node = lock.get_mut(filename).unwrap();
        match node {
            Node::File(entry) => {
                entry.name = name.to_string();
            }
            _ => panic!("Cant rename!"),
        }
        //current
        //    .lock()
        //    .unwrap()
        //    .insert(filename.to_string(), Node::File());
    }

    pub fn move_node(&mut self, old_path: &str, new_path: &str) {
        let node = self.get(old_path).unwrap();
        self.remove(old_path);
        match node {
            Node::File(file) => {
                self.insert_file(new_path, file);
            }
            Node::Directory(dir) => {
                self.insert_directory(new_path, Some(Node::Directory(dir)));
            }
        }
    }

    pub fn is_file(&self, path: &str) -> bool {
        let mut current = self.0.clone();

        current = Self::get_root(current);

        let (curr, filename) = Self::get_last_two_nodes(current, path);
        current = curr;

        let child = current.lock().unwrap().get(filename).unwrap().clone();

        match child {
            Node::Directory(_) => false,
            Node::File(_) => true,
        }
    }

    pub fn pretty_print(&self) {
        pretty_print_helper(&self.0.lock().unwrap().clone(), 0);
    }

    fn get_root(current: Arc<Mutex<BTreeMap<String, Node>>>) -> Arc<Mutex<BTreeMap<String, Node>>> {
        let child = current.lock().unwrap().get("").unwrap().clone();

        match child {
            Node::Directory(dir) => dir,
            _ => panic!("Path is not valid"),
        }
    }

    fn get_or_insert_root(
        current: Arc<Mutex<BTreeMap<String, Node>>>,
    ) -> Arc<Mutex<BTreeMap<String, Node>>> {
        let child = current
            .lock()
            .unwrap()
            .entry("".to_string())
            .or_default()
            .clone();

        match child {
            Node::Directory(dir) => dir,
            _ => panic!("Path is not valid"),
        }
    }

    fn get_last_two_nodes(
        mut current: Arc<Mutex<BTreeMap<String, Node>>>,
        path: &str,
    ) -> (Arc<Mutex<BTreeMap<String, Node>>>, &str) {
        let parts = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let parts_len = parts.len() - 1;

        for part in parts.iter().take(parts_len) {
            let child = current.lock().unwrap().get(*part).cloned();

            current = match child {
                Some(Node::Directory(dir)) => dir,
                _ => panic!("Path is not valid"),
            };
        }

        (current, parts.last().unwrap())
    }

    fn get_or_insert_last_two_nodes(
        mut current: Arc<Mutex<BTreeMap<String, Node>>>,
        path: &str,
    ) -> (Arc<Mutex<BTreeMap<String, Node>>>, &str) {
        let parts = path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let parts_len = parts.len() - 1;

        for part in parts.iter().take(parts_len) {
            let child = current
                .lock()
                .unwrap()
                .entry(part.to_string())
                .or_default()
                .clone();

            current = match child {
                Node::Directory(dir) => dir,
                _ => panic!("Path is not valid"),
            };
        }
        (current, parts.last().unwrap())
    }
}

pub fn pretty_print_helper(node: &BTreeMap<String, Node>, indent: usize) {
    for (name, child) in node {
        match child {
            Node::Directory(dir) => {
                println!("{:indent$}|- {name}/", "", indent = indent * 2, name = name);
                pretty_print_helper(&dir.lock().unwrap(), indent + 1);
            }
            Node::File(file) => {
                println!(
                    "{:indent$}|- {name} : {f}",
                    "",
                    indent = indent * 2,
                    name = name,
                    f = file.size
                );
            }
        }
    }
}
