namespace Samples.Interop;

public interface IReadable
{
    int Read();

    int Value { get; }
}

public interface ILeft : IReadable { }
public interface IRight : IReadable { }
public interface ICounter : ILeft, IRight { }

public interface IWritable
{
    int Value { get; set; }
}

public class Counter : ICounter, IWritable
{
    private int value;

    public Counter(int start) => value = start;

    public virtual int Value { get => value; set => this.value = value; }

    public int Snapshot { get => value; private set => this.value = value; }

    public int Initial { get; init; }

    public int this[int index] => index + value;

    public void Add(int amount) => value += amount;

    public long Add(long amount) => value + amount;

    public Counter Copy() => new Counter(value);

    public virtual int Read() => value;

    private int Hidden() => value;
}

public sealed class DoubleCounter : Counter
{
    public DoubleCounter(int start) : base(start) { }

    public override int Read() => 2 * base.Read();

    public override int Value { get => 2 * base.Value; set => base.Value = value; }

    public int Add(string value) => value.Length;
}

public sealed class FixedCounter : ICounter
{
    public int Read() => 30;

    public int Value => Read();
}

public static class CounterLabel
{
    private static readonly Counter tracked = new Counter(0);

    public static int Reads { get; set; }

    public static Counter Tracked
    {
        get
        {
            Reads++;
            return tracked;
        }
    }

    public static int Last { private get; set; }

    public static int ReadLast() => Last;

    public static T Keep<T>(T counter) where T : Counter, IWritable => counter;

    public static int ReadBound<T>(T counter) where T : IReadable => counter.Value;

    public static string Rank<T>(T counter, int _) where T : Counter => "constrained";

    public static string Rank<T>(T counter, long _) => "open";

    public static string Of(IReadable value) => "readable";

    public static string Of(ICounter value) => "counter";

    public static string Of(Counter value) => "class";

    public static string Choose(ILeft value) => "left";

    public static string Choose(IRight value) => "right";
}
