static class Program
{
    static int Work(int iterations, string left, string right)
    {
        int checksum = 0;
        int i = 0;
        while (i < iterations)
        {
            string text = $"{left}:{right}";
            checksum += text.Length;
            i += 1;
        }
        return checksum;
    }

    static void Main()
    {
        string left = "key";
        string right = "42";
        Work(200000, left, right);
        long before = System.GC.GetAllocatedBytesForCurrentThread();
        long start = System.Diagnostics.Stopwatch.GetTimestamp();
        int checksum = Work(1000000, left, right);
        long elapsed = System.Diagnostics.Stopwatch.GetTimestamp() - start;
        long bytes = System.GC.GetAllocatedBytesForCurrentThread() - before;
        System.Console.WriteLine(checksum);
        System.Console.WriteLine(elapsed);
        System.Console.WriteLine(System.Diagnostics.Stopwatch.Frequency);
        System.Console.WriteLine(bytes);
    }
}
