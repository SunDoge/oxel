static class Program
{
    static int Kernel(int seed)
    {
        int state = seed;
        int i = 0;
        while (i < 1000000)
        {
            state = (state * 17 + i) % 251;
            i += 1;
        }
        return state;
    }

    static void Main()
    {
        int checksum = 0;
        int pass = 0;
        while (pass < 8)
        {
            checksum += Kernel(pass);
            pass += 1;
        }
        long start = System.Diagnostics.Stopwatch.GetTimestamp();
        while (pass < 40)
        {
            checksum += Kernel(pass);
            pass += 1;
        }
        long elapsed = System.Diagnostics.Stopwatch.GetTimestamp() - start;
        System.Console.WriteLine(checksum);
        System.Console.WriteLine(elapsed);
        System.Console.WriteLine(System.Diagnostics.Stopwatch.Frequency);
    }
}
