/**
 * @file test.cpp
 * @brief Test file for pretty-hover plugin demonstration
 * 
 * This file contains various Doxygen documentation styles to test
 * the pretty-hover plugin's transformation capabilities.
 */

#include <iostream>
#include <vector>

/**
 * @class Calculator
 * @brief A simple calculator class for demonstration
 * 
 * This class provides basic arithmetic operations and demonstrates
 * how the pretty-hover plugin transforms Doxygen documentation.
 */
class Calculator {
public:
    /**
     * @brief Adds two numbers together
     * @param a The first operand
     * @param b The second operand
     * @return The sum of a and b
     * 
     * @note This is a simple addition operation
     * @see subtract()
     */
    int add(int a, int b) {
        return a + b;
    }
    
    /**
     * @brief Subtracts second number from first
     * @param a The minuend
     * @param b The subtrahend
     * @return The difference of a and b
     * @warning Be careful with negative results
     */
    int subtract(int a, int b) {
        return a - b;
    }
    
    /**
     * @brief Divides two numbers
     * @param numerator The dividend
     * @param denominator The divisor
     * @return The quotient of numerator and denominator
     * @error Division by zero will cause undefined behavior
     * @throw std::invalid_argument if denominator is zero
     */
    double divide(int numerator, int denominator) {
        if (denominator == 0) {
            throw std::invalid_argument("Division by zero");
        }
        return static_cast<double>(numerator) / denominator;
    }
    
    /**
     * @brief Multiplies two numbers
     * @param x First factor
     * @param y Second factor
     * @retval int The product of x and y
     * 
     * Example usage:
     * @code{cpp}
     * Calculator calc;
     * int result = calc.multiply(5, 3);  // result = 15
     * @endcode
     */
    int multiply(int x, int y) {
        return x * y;
    }
};

/**
 * @brief Processes a list of items
 * @tparam T The type of items in the list
 * @param items Vector of items to process
 * @return Number of items processed
 * 
 * Processing steps:
 * @li Validate input
 * @li Process each item
 * @li Return count
 * 
 * @remark This is a template function that works with any type
 */
template<typename T>
size_t processItems(const std::vector<T>& items) {
    return items.size();
}

/**
 * @brief Main entry point
 * @return Exit code (0 for success)
 * 
 * Test number conversions by hovering over these values:
 * - Decimal: 255
 * - Hex: 0xFF
 * - Octal: 0377
 * - Binary: 0b11111111
 */
int main() {
    Calculator calc;
    
    int sum = calc.add(10, 20);
    int product = calc.multiply(5, 6);
    
    std::cout << "Sum: " << sum << std::endl;
    std::cout << "Product: " << product << std::endl;
    
    return 0;
}
