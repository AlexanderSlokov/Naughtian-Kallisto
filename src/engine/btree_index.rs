use std::mem;

#[derive(Clone)]
pub struct Node {
    pub is_leaf_node: bool,
    pub path_keys: Vec<String>,
    pub child_nodes: Vec<Box<Node>>,
}

impl Node {
    pub fn new(is_leaf: bool) -> Self {
        Self {
            is_leaf_node: is_leaf,
            path_keys: Vec::new(),
            child_nodes: Vec::new(),
        }
    }
}

/// Simplified B-Tree for strings (paths).
/// Acts as a validator before secret lookup in the CuckooTable.
#[derive(Clone)]
pub struct BTreeIndex {
    root_node: Box<Node>,
    min_degree: usize,
}

impl BTreeIndex {
    pub fn new(degree: usize) -> Self {
        Self {
            root_node: Box::new(Node::new(true)),
            min_degree: degree,
        }
    }

    pub fn insert_path(&mut self, path: &str) -> bool {
        if self.validate_path(path) {
            return true;
        }

        if self.root_node.path_keys.len() == 2 * self.min_degree - 1 {
            let mut new_root = Box::new(Node::new(false));
            let old_root = mem::replace(&mut self.root_node, Box::new(Node::new(false)));
            new_root.child_nodes.push(old_root);
            self.root_node = new_root;
            self.split_child_node(&mut *self.root_node, 0);
            self.insert_into_non_full_node(&mut *self.root_node, path);
        } else {
            self.insert_into_non_full_node(&mut *self.root_node, path);
        }
        true
    }

    pub fn validate_path(&self, path: &str) -> bool {
        self.contains_path_recursive(&self.root_node, path)
    }
    
    pub fn get_all_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        self.collect_paths_recursive(&self.root_node, &mut paths);
        paths
    }
    
    fn collect_paths_recursive(&self, node: &Node, paths: &mut Vec<String>) {
        if node.is_leaf_node {
            for key in &node.path_keys {
                paths.push(key.clone());
            }
        } else {
            for i in 0..node.path_keys.len() {
                self.collect_paths_recursive(&node.child_nodes[i], paths);
                paths.push(node.path_keys[i].clone());
            }
            self.collect_paths_recursive(&node.child_nodes.last().unwrap(), paths);
        }
    }

    fn contains_path_recursive(&self, current_node: &Node, path_key: &str) -> bool {
        let mut index = 0;
        while index < current_node.path_keys.len() && path_key > current_node.path_keys[index].as_str() {
            index += 1;
        }

        if index < current_node.path_keys.len() && current_node.path_keys[index] == path_key {
            return true;
        }

        if current_node.is_leaf_node {
            return false;
        }

        self.contains_path_recursive(&current_node.child_nodes[index], path_key)
    }

    fn insert_into_non_full_node(&mut self, current_node: &mut Node, path_key: &str) {
        let mut index = current_node.path_keys.len() as isize - 1;

        if current_node.is_leaf_node {
            current_node.path_keys.push(String::new());
            while index >= 0 && path_key < current_node.path_keys[index as usize].as_str() {
                current_node.path_keys[(index + 1) as usize] = current_node.path_keys[index as usize].clone();
                index -= 1;
            }
            current_node.path_keys[(index + 1) as usize] = path_key.to_string();
        } else {
            while index >= 0 && path_key < current_node.path_keys[index as usize].as_str() {
                index -= 1;
            }
            index += 1;
            
            let u_index = index as usize;
            if current_node.child_nodes[u_index].path_keys.len() == 2 * self.min_degree - 1 {
                self.split_child_node(current_node, u_index);
                if path_key > current_node.path_keys[u_index].as_str() {
                    index += 1;
                }
            }
            self.insert_into_non_full_node(&mut current_node.child_nodes[index as usize], path_key);
        }
    }

    fn split_child_node(&mut self, parent_node: &mut Node, child_index: usize) {
        let child_node = &mut parent_node.child_nodes[child_index];
        let mut new_sibling_node = Box::new(Node::new(child_node.is_leaf_node));

        for _ in 0..(self.min_degree - 1) {
            new_sibling_node.path_keys.push(child_node.path_keys[self.min_degree].clone());
            child_node.path_keys.remove(self.min_degree);
        }

        if !child_node.is_leaf_node {
            for _ in 0..self.min_degree {
                new_sibling_node.child_nodes.push(child_node.child_nodes.remove(self.min_degree));
            }
        }

        let middle_key = child_node.path_keys.remove(self.min_degree - 1);

        parent_node.child_nodes.insert(child_index + 1, new_sibling_node);
        parent_node.path_keys.insert(child_index, middle_key);
    }
}
