#include <gtest/gtest.h>
#include <filesystem>
// Disable main() in kallisto_server.cpp so we can unit test its internal classes
#define KALLISTO_TEST_MODE
#include "kallisto_server.cpp"

using namespace kallisto;

// -----------------------------------------------------------------------------
// KALLISTO SERVER CONFIG TEST SUITE
// Problem Description: Server logic is prone to unparsed CLI arguments and unhandled
// edge cases during boot-up or shutdown.
// Goals:
// - Coverage of CLI parsing logic 
// - Test boundary values (custom configs and default configs)
// -----------------------------------------------------------------------------

TEST(KallistoServerTest, ParseDefaultArgs) {
    int argc = 1;
    char* argv[] = { (char*)"kallisto_server" };
    
    ServerConfig config = ServerConfig::parseFromArgs(argc, argv);
    EXPECT_EQ(config.http_port, 8200);
    EXPECT_EQ(config.db_path, "/kallisto/data");
    
    if (std::thread::hardware_concurrency() != 0) {
        EXPECT_EQ(config.num_workers, std::thread::hardware_concurrency());
    }
}

TEST(KallistoServerTest, ParseCustomArgs) {
    int argc = 4;
    char* argv[] = { 
        (char*)"kallisto_server", 
        (char*)"--http-port=9999",
        (char*)"--workers=12",
        (char*)"--db-path=/mock/path/db"
    };
    
    ServerConfig config = ServerConfig::parseFromArgs(argc, argv);
    EXPECT_EQ(config.http_port, 9999);
    EXPECT_EQ(config.num_workers, 12);
    EXPECT_EQ(config.db_path, "/mock/path/db");
}

TEST(KallistoServerTest, CheckHelpFlag) {
    int argc = 2;
    char* argv[] = { (char*)"kallisto_server", (char*)"--help" };
    ServerConfig config = ServerConfig::parseFromArgs(argc, argv);
    EXPECT_TRUE(config.isHelpRequested(argc, argv));
    
    char* argv2[] = { (char*)"kallisto_server", (char*)"-h" };
    EXPECT_TRUE(config.isHelpRequested(argc, argv2));
}

TEST(KallistoServerTest, CheckNoHelpFlag) {
    int argc = 2;
    char* argv[] = { (char*)"kallisto_server", (char*)"--workers=4" };
    ServerConfig config = ServerConfig::parseFromArgs(argc, argv);
    EXPECT_FALSE(config.isHelpRequested(argc, argv));
}

TEST(KallistoServerTest, LifecycleSanityCheck) {
    // Avoid port conflicts and pollution of system DB path
    ServerConfig config;
    config.http_port = 12000;
    config.db_path = "/tmp/kallisto_test_db_server";
    config.num_workers = 1;
    
    {
        // Simply constructing the App initializes the KallistoCore and Dispatcher
        EXPECT_NO_THROW({
            KallistoServerApp app(config);
            app.shutdown(); // Manually initiate shut down procedures (flushing)
        });
    }
}



TEST(KallistoServerTest, ConfigOutputPrinting) {
    // Problem Description: Test help output and banner printing to ensure no crashes
    ServerConfig config;
    EXPECT_NO_THROW({
        config.printHelpInstructions();
        config.printBanner();
    });
}

TEST(KallistoServerTest, FullLifecycleAndSignals) {
    // Problem Description: Simulate a full server boot up, binding listeners,
    // and shutting down via an OS signal (SIGINT). This tests bindHttpListeners,
    // start, waitForShutdown, and the signalHandler.
    ServerConfig config;
    config.http_port = 13500;
    config.db_path = "/tmp/kallisto_test_lifecycle_db";
    config.num_workers = 2;
    
    // Ensure clean state
    std::filesystem::remove_all(config.db_path);

    KallistoServerApp app(config);
    
    // Start workers and bind HTTP endpoints
    EXPECT_NO_THROW(app.start());
    
    // Run waitForShutdown in a separate thread so it blocks
    std::atomic<bool> wait_finished{false};
    std::thread wait_thread([&]() {
        app.waitForShutdown();
        wait_finished = true;
    });
    
    // Simulate OS sending SIGTERM to the process
    signalHandler(SIGTERM);
    
    // Wait for shutdown to process
    wait_thread.join();
    EXPECT_TRUE(wait_finished.load());
    
    // Finish shutdown
    EXPECT_NO_THROW(app.shutdown());
}
