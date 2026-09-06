using System.Diagnostics;
using System.Text;
using Reshot.Core.Diagnostics;
using Xunit;
using Xunit.Abstractions;

namespace Reshot.Core.Tests;

public sealed class LogPerfTests
{
    private readonly ITestOutputHelper _output;

    public LogPerfTests(ITestOutputHelper output) => _output = output;

    [Fact]
    public void Log_writes_and_flushes_to_file()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_{Guid.NewGuid():N}.log");
        try
        {
            Log.SetLogFile(tempFile);
            Log.Info("Test Info Message");
            Log.Warn("Test Warn Message");
            Log.Error("Test Error Message");
            Log.SetLogFile(null); // flush and close so ReadAllText can access

            var content = File.ReadAllText(tempFile);
            Assert.Contains("[Info ] Test Info Message", content);
            Assert.Contains("[Warn ] Test Warn Message", content);
            Assert.Contains("[Error] Test Error Message", content);
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    [Fact]
    public void Log_is_thread_safe_under_concurrency()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_conc_{Guid.NewGuid():N}.log");
        try
        {
            Log.SetLogFile(tempFile);
            const int threads = 8;
            const int linesPerThread = 200;

            Parallel.For(0, threads, threadId =>
            {
                for (int i = 0; i < linesPerThread; i++)
                {
                    Log.Info($"Thread {threadId} line {i}");
                }
            });

            Log.SetLogFile(null); // flush and close

            var lines = File.ReadAllLines(tempFile);
            Assert.Equal(threads * linesPerThread, lines.Length);
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    [Fact]
    public void Log_trims_when_exceeding_1MB()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_trim_{Guid.NewGuid():N}.log");
        try
        {
            Log.SetLogFile(tempFile);
            // Write large block of data > 1 MB
            var largeLine = new string('A', 10000);
            for (int i = 0; i < 110; i++)
            {
                Log.Info(largeLine);
            }

            Log.SetLogFile(null);
            // File should have been trimmed in-place
            var length = new FileInfo(tempFile).Length;
            Assert.True(length < 1_000_000, $"File size should be < 1MB after trim, but was {length}");
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    [Fact]
    public void Log_rollover_contains_no_null_bytes_and_retains_latest_lines()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_rollover_{Guid.NewGuid():N}.log");
        try
        {
            Log.SetLogFile(tempFile);
            // Write until rollover occurs (each line is ~10 KB)
            var largePayload = new string('A', 10000);
            for (int i = 0; i < 110; i++)
            {
                Log.Info($"Iteration {i:D4}: {largePayload}");
            }

            // Write final lines after rollover
            Log.Info("Triggering Line After Rollover");
            Log.Info("Final Verification Line");

            Log.SetLogFile(null); // flush and close

            // 1. Assert raw bytes contain NO null bytes
            var bytes = File.ReadAllBytes(tempFile);
            Assert.DoesNotContain((byte)0, bytes);

            // 2. Assert file size is under the 1 MB cap
            Assert.True(bytes.Length < 1_000_000, $"File size should be < 1MB after rollover, was {bytes.Length}");

            // 3. Assert file starts at a clean line boundary
            var lines = File.ReadAllLines(tempFile);
            Assert.NotEmpty(lines);
            Assert.Matches(@"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} \[[A-Za-z ]+\] ", lines[0]);

            // 4. Assert last lines written are present and readable
            Assert.Contains(lines, l => l.Contains("Triggering Line After Rollover"));
            Assert.Contains(lines, l => l.Contains("Final Verification Line"));
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    [Fact]
    public void Log_rollover_twice_in_a_row_remains_clean()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_double_rollover_{Guid.NewGuid():N}.log");
        try
        {
            Log.SetLogFile(tempFile);
            var payload = new string('B', 10000);

            // Pass 1: trigger first rollover
            for (int i = 0; i < 110; i++)
            {
                Log.Info($"Pass1_{i:D4}: {payload}");
            }
            Log.Info("Marker between pass 1 and pass 2");

            // Pass 2: trigger second rollover
            for (int i = 0; i < 110; i++)
            {
                Log.Info($"Pass2_{i:D4}: {payload}");
            }
            Log.Info("Final Line After Double Rollover");

            Log.SetLogFile(null); // flush and close

            var bytes = File.ReadAllBytes(tempFile);
            Assert.DoesNotContain((byte)0, bytes);
            Assert.True(bytes.Length < 1_000_000, $"File size should be < 1MB after double rollover, was {bytes.Length}");

            var lines = File.ReadAllLines(tempFile);
            Assert.NotEmpty(lines);
            Assert.Matches(@"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} \[[A-Za-z ]+\] ", lines[0]);
            Assert.Contains(lines, l => l.Contains("Final Line After Double Rollover"));
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    [Fact]
    public void Log_open_existing_large_file_truncates_without_null_bytes()
    {
        var tempFile = Path.Combine(Path.GetTempPath(), $"reshot_test_log_existing_{Guid.NewGuid():N}.log");
        try
        {
            // Create dummy file > 1 MB on disk
            var dummy = new byte[1_050_000];
            Array.Fill(dummy, (byte)'Z');
            File.WriteAllBytes(tempFile, dummy);

            Log.SetLogFile(tempFile);
            Log.Info("Line written into truncated file");
            Log.SetLogFile(null);

            var bytes = ReadAllBytesWithRetry(tempFile);
            Assert.DoesNotContain((byte)0, bytes);
            Assert.True(bytes.Length < 1_000_000, $"Length should be < 1MB, was {bytes.Length}");

            var text = Encoding.UTF8.GetString(bytes);
            Assert.Contains("Line written into truncated file", text);
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFile);
        }
    }

    private static byte[] ReadAllBytesWithRetry(string path)
    {
        for (int i = 0; i < 20; i++)
        {
            try
            {
                using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite);
                using var ms = new MemoryStream();
                fs.CopyTo(ms);
                return ms.ToArray();
            }
            catch (IOException)
            {
                Thread.Sleep(50);
            }
        }
        return File.ReadAllBytes(path);
    }

    private static void TryDelete(string path)
    {
        for (int i = 0; i < 10; i++)
        {
            try
            {
                if (File.Exists(path)) File.Delete(path);
                return;
            }
            catch (IOException)
            {
                Thread.Sleep(50);
            }
        }
    }

    [Fact]
    public void Interpolated_string_handler_skips_evaluation_when_disabled()
    {
        bool evaluated = false;
        string ExpensiveFormat()
        {
            evaluated = true;
            return "expensive";
        }

        try
        {
            Log.SetLogFile(null);
            Log.MinimumLevel = Log.Level.Error;

            Log.Info($"Should not evaluate: {ExpensiveFormat()}");

            Assert.False(evaluated, "Hole in interpolated string should NOT be evaluated when log level is disabled");
        }
        finally
        {
            Log.MinimumLevel = Log.Level.Info;
        }
    }

    [Fact]
    public void Benchmark_before_vs_after()
    {
        var tempFileBaseline = Path.Combine(Path.GetTempPath(), $"reshot_bench_base_{Guid.NewGuid():N}.log");
        var tempFileOptimized = Path.Combine(Path.GetTempPath(), $"reshot_bench_opt_{Guid.NewGuid():N}.log");

        const int iterations = 1000;

        try
        {
            // 1. Measure Baseline (File.AppendAllText + DateTime.Now string interpolation)
            File.AppendAllText(tempFileBaseline, "warmup\n", Encoding.UTF8);

            var sw = Stopwatch.StartNew();
            long gcBeforeBase = GC.GetAllocatedBytesForCurrentThread();

            for (int i = 0; i < iterations; i++)
            {
                var line = $"{DateTime.Now:yyyy-MM-dd HH:mm:ss.fff} [Info ] Message iteration {i}";
                File.AppendAllText(tempFileBaseline, line + Environment.NewLine, Encoding.UTF8);
            }

            sw.Stop();
            long gcAfterBase = GC.GetAllocatedBytesForCurrentThread();
            double baseTotalMs = sw.Elapsed.TotalMilliseconds;
            double basePerLineUs = (baseTotalMs / iterations) * 1000.0;
            long baseAllocPerLine = (gcAfterBase - gcBeforeBase) / iterations;

            // 2. Measure Optimized (Log.Info with StreamWriter AutoFlush + Span timestamp)
            Log.SetLogFile(tempFileOptimized);
            Log.Info("warmup");

            sw.Restart();
            long gcBeforeOpt = GC.GetAllocatedBytesForCurrentThread();

            for (int i = 0; i < iterations; i++)
            {
                Log.Info($"Message iteration {i}");
            }

            sw.Stop();
            long gcAfterOpt = GC.GetAllocatedBytesForCurrentThread();
            double optTotalMs = sw.Elapsed.TotalMilliseconds;
            double optPerLineUs = (optTotalMs / iterations) * 1000.0;
            long optAllocPerLine = (gcAfterOpt - gcBeforeOpt) / iterations;

            _output.WriteLine($"--- LOG BENCHMARK RESULTS (1000 lines) ---");
            _output.WriteLine($"BEFORE (AppendAllText): {baseTotalMs:F2} ms total | {basePerLineUs:F1} us/line | {baseAllocPerLine} B/line allocated");
            _output.WriteLine($"AFTER  (StreamWriter) : {optTotalMs:F2} ms total | {optPerLineUs:F1} us/line | {optAllocPerLine} B/line allocated");
            _output.WriteLine($"SPEEDUP: {baseTotalMs / Math.Max(0.001, optTotalMs):F1}x faster");
            _output.WriteLine($"ALLOCATION REDUCTION: {baseAllocPerLine - optAllocPerLine} B saved per line ({(double)(baseAllocPerLine - optAllocPerLine)/baseAllocPerLine*100:F1}%)");
        }
        finally
        {
            Log.SetLogFile(null);
            TryDelete(tempFileBaseline);
            TryDelete(tempFileOptimized);
        }
    }
}
