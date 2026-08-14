use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

/// Helper function to run a command and return stdout
fn run_vault_cmd(args: &[&str]) -> String {
    let output = Command::new("docker")
        .arg("compose")
        .arg("-f")
        .arg("tests/e2e/docker-compose.test.yml")
        .arg("exec")
        .arg("-T")
        .arg("vault-cli")
        .arg("vault")
        .args(args)
        .output()
        .expect("Failed to execute docker compose exec vault");
        
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    
    if !output.status.success() {
        eprintln!("Vault command failed. Stderr: {}", stderr);
        eprintln!("Stdout: {}", stdout);
    }
    
    assert!(output.status.success(), "Vault command failed: vault {}", args.join(" "));
    stdout
}

#[tokio::test]
#[ignore]
async fn test_vault_e2e_compat() {
    // 1. Bring up the environment
    println!("Starting e2e docker-compose environment...");
    let up_status = Command::new("docker")
        .arg("compose")
        .arg("-f")
        .arg("tests/e2e/docker-compose.test.yml")
        .arg("up")
        .arg("-d")
        .arg("--build")
        .status()
        .expect("Failed to start docker-compose");
    assert!(up_status.success());
    
    // Give it a few seconds to fully initialize
    sleep(Duration::from_secs(5)).await;
    
    // Run the tests
    let res = tokio::task::spawn_blocking(|| {
        run_e2e_sequence();
    }).await;
    
    // 3. Tear down
    println!("Tearing down e2e docker-compose environment...");
    Command::new("docker")
        .arg("compose")
        .arg("-f")
        .arg("tests/e2e/docker-compose.test.yml")
        .arg("down")
        .arg("-v")
        .status()
        .expect("Failed to teardown docker-compose");
        
    res.expect("E2E tests panicked");
}

fn run_e2e_sequence() {
    println!("1. Putting initial secret...");
    run_vault_cmd(&["kv", "put", "secret/app/db", "user=admin", "pass=s3cr3t"]);
    
    println!("2. Getting latest secret...");
    let out = run_vault_cmd(&["kv", "get", "secret/app/db"]);
    assert!(out.contains("user"));
    assert!(out.contains("admin"));
    assert!(out.contains("pass"));
    assert!(out.contains("s3cr3t"));
    
    println!("3. Putting a second version...");
    run_vault_cmd(&["kv", "put", "secret/app/db", "user=admin", "pass=new_s3cr3t"]);
    
    println!("4. Getting version 1 (query param)...");
    let out_v1 = run_vault_cmd(&["kv", "get", "-version=1", "secret/app/db"]);
    assert!(out_v1.contains("s3cr3t"));
    assert!(!out_v1.contains("new_s3cr3t"));
    
    println!("5. Patching the secret...");
    run_vault_cmd(&["kv", "patch", "secret/app/db", "host=localhost"]);
    
    println!("6. Getting latest secret again (should be merged)...");
    let out_merged = run_vault_cmd(&["kv", "get", "secret/app/db"]);
    assert!(out_merged.contains("user"));
    assert!(out_merged.contains("admin"));
    assert!(out_merged.contains("pass"));
    assert!(out_merged.contains("new_s3cr3t"));
    assert!(out_merged.contains("host"));
    assert!(out_merged.contains("localhost"));
    
    println!("7. Checking metadata...");
    let out_meta = run_vault_cmd(&["kv", "metadata", "get", "secret/app/db"]);
    // Should show 3 versions now
    assert!(out_meta.contains("3")); 
    
    println!("8. Deleting version 1...");
    run_vault_cmd(&["kv", "delete", "-versions=1", "secret/app/db"]);
    
    // Get version 1 should fail or show deleted
    let out_v1_deleted = Command::new("docker")
        .arg("compose").arg("-f").arg("tests/e2e/docker-compose.test.yml")
        .arg("exec").arg("-T").arg("vault-cli").arg("vault")
        .arg("kv").arg("get").arg("-version=1").arg("secret/app/db")
        .output().unwrap();
    assert!(!out_v1_deleted.status.success() || String::from_utf8_lossy(&out_v1_deleted.stderr).contains("deleted"));
    
    println!("9. Undeleting version 1...");
    run_vault_cmd(&["kv", "undelete", "-versions=1", "secret/app/db"]);
    
    println!("10. Destroying version 1...");
    run_vault_cmd(&["kv", "destroy", "-versions=1", "secret/app/db"]);
    
    // List keys
    println!("11. Putting another key to list...");
    run_vault_cmd(&["kv", "put", "secret/app/web", "url=http://example.com"]);
    
    println!("12. Listing keys...");
    let out_list = run_vault_cmd(&["kv", "list", "secret/app/"]);
    assert!(out_list.contains("db"));
    assert!(out_list.contains("web"));
    
    println!("All E2E tests passed!");
}
