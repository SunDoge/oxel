using System;
using Microsoft.ML.OnnxRuntime;

using var session = new InferenceSession(args[0]);
using var options = new RunOptions();

float[] Predict(float[] input)
{
    long[] shape = [3];
    var output = new float[3];
    using var inputTensor = OrtValue.CreateTensorValueFromMemory(input, shape);
    using var outputTensor = OrtValue.CreateTensorValueFromMemory(output, shape);
    session.Run(options, ["input"], [inputTensor], ["output"], [outputTensor]);
    return output;
}

foreach (var input in new float[][] { [1, 2, 3], [-2, 0, 4] })
{
    foreach (var value in Predict(input))
        Console.WriteLine(value);
}
